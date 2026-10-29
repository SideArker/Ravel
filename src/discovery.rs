use crate::store::{validate_fingerprint, validate_name};
use anyhow::{Result, ensure};
use mdns_sd::{IfKind, ServiceDaemon, ServiceEvent, ServiceInfo};
use std::{
    collections::BTreeMap,
    net::{IpAddr, SocketAddr},
    time::Duration,
};
use tokio::sync::mpsc;

pub const SERVICE: &str = "_ravel._tcp.local.";
#[derive(Clone, Debug)]
pub struct Peer {
    pub service: String,
    pub name: String,
    pub fingerprint: String,
    pub addresses: Vec<SocketAddr>,
}
pub struct Discovery {
    daemon: ServiceDaemon,
    fullname: String,
}

pub fn lan_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => v.is_private() || v.is_link_local(),
        _ => false,
    }
}
pub async fn resolve(address: &str) -> Result<Vec<SocketAddr>> {
    ensure!(
        address.len() <= 255 && !address.chars().any(char::is_control),
        "Invalid host:port"
    );
    let addresses =
        tokio::time::timeout(Duration::from_secs(5), tokio::net::lookup_host(address)).await??;
    let mut addresses: Vec<_> = addresses
        .filter(|s| s.port() != 0 && lan_ip(s.ip()))
        .collect();
    addresses.sort();
    addresses.dedup();
    ensure!(
        !addresses.is_empty(),
        "Address must resolve to a private/link-local IPv4 LAN endpoint"
    );
    Ok(addresses)
}
impl Discovery {
    pub fn new(name: &str, fp: &str, port: u16) -> Result<Self> {
        validate_name(name)?;
        validate_fingerprint(fp)?;
        let daemon = ServiceDaemon::new()?;
        daemon.disable_interface(IfKind::IPv6)?;
        daemon.set_ip_check_interval(2)?;
        let instance = format!("Ravel-{}", &fp[..16]);
        let hostname = format!("ravel-{}.local.", &fp[..16]);
        let info = ServiceInfo::new(
            SERVICE,
            &instance,
            &hostname,
            "",
            port,
            &[("name", name), ("fp", fp), ("version", "1")][..],
        )?
        .enable_addr_auto();
        let fullname = info.get_fullname().to_owned();
        daemon.register(info)?;
        Ok(Self { daemon, fullname })
    }
    pub async fn run(self, own_fp: String, updates: mpsc::Sender<Vec<Peer>>) -> Result<()> {
        let receiver = self.daemon.browse(SERVICE)?;
        let mut peers: BTreeMap<String, Peer> = BTreeMap::new();
        let mut refresh = tokio::time::interval(Duration::from_secs(15));
        loop {
            tokio::select! {
                _=updates.closed()=>return Ok(()),
                _=refresh.tick()=>{
                    for key in peers.keys() { let _=self.daemon.verify(key.clone(),Duration::from_secs(3)); }
                },
                event=receiver.recv_async()=>{
                    match event? {
                        ServiceEvent::ServiceResolved(info)=>{
                            let name=info.get_property_val_str("name").unwrap_or("");
                            let fp=info.get_property_val_str("fp").unwrap_or("");
                            if fp==own_fp || validate_name(name).is_err() || validate_fingerprint(fp).is_err() || info.get_property_val_str("version")!=Some("1") {continue;}
                            let addresses=info.get_addresses_v4().into_iter().map(|ip|SocketAddr::new(IpAddr::V4(ip),info.port)).filter(|addr|lan_ip(addr.ip()) && addr.port()!=0).take(8).collect::<Vec<_>>();
                            if addresses.is_empty() || (peers.len()>=64 && !peers.contains_key(&info.fullname)) {continue;}
                            peers.insert(info.fullname.clone(),Peer{service:info.fullname.clone(),name:name.into(),fingerprint:fp.into(),addresses});
                        },
                        ServiceEvent::ServiceRemoved(_,name)=>{peers.remove(&name);},
                        _=>continue,
                    }
                    // Snapshots can be coalesced; authentication never uses this cache.
                    let _=updates.try_send(peers.values().cloned().collect());
                }
            }
        }
    }
}
impl Drop for Discovery {
    fn drop(&mut self) {
        // Exit alone does not withdraw advertised records. Commands are FIFO.
        let _ = self.daemon.unregister(&self.fullname);
        let _ = self.daemon.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn manual_entry_rejects_public_and_loopback_addresses() {
        assert!(resolve("8.8.8.8:4243").await.is_err());
        assert!(resolve("127.0.0.1:4243").await.is_err());
        assert!(resolve("192.168.1.2:0").await.is_err());
        assert_eq!(resolve("192.168.1.2:4243").await.unwrap().len(), 1);
    }
}

#[cfg(test)]
mod native_tests {
    use super::*;
    #[tokio::test]
    #[ignore = "requires LAN multicast; publishes two temporary Ravel discovery services"]
    async fn lan_discovery_resolves_and_removes_services() {
        let a = hex::encode(rand::random::<[u8; 32]>());
        let b = hex::encode(rand::random::<[u8; 32]>());
        let da = Discovery::new("Discovery check A", &a, 30201).unwrap();
        let db = Discovery::new("Discovery check B", &b, 30202).unwrap();
        let (ta, mut ra) = mpsc::channel(32);
        let (tb, _rb) = mpsc::channel(32);
        let aa = a.clone();
        let bb = b.clone();
        let task_a = tokio::spawn(async move {
            da.run(aa, ta).await.unwrap();
        });
        let task_b = tokio::spawn(async move {
            db.run(bb, tb).await.unwrap();
        });
        let resolved = tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                let peers = ra.recv().await.unwrap();
                if let Some(peer) = peers.iter().find(|p| p.fingerprint == b) {
                    assert_eq!(peer.name, "Discovery check B");
                    assert!(
                        peer.addresses
                            .iter()
                            .all(|a| lan_ip(a.ip()) && a.port() == 30202)
                    );
                    break;
                }
            }
        })
        .await;
        task_b.abort();
        let _ = task_b.await;
        let removed = tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                let peers = ra.recv().await.unwrap();
                if peers.iter().all(|p| p.fingerprint != b) {
                    break;
                }
            }
        })
        .await;
        task_a.abort();
        let _ = task_a.await;
        assert!(resolved.is_ok(), "LAN multicast discovery failed");
        assert!(removed.is_ok(), "Discovery removal failed");
    }
}
