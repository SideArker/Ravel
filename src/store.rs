use crate::{
    geometry::Layout,
    identity::{atomic_private, read_private},
};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrustedPeer {
    pub name: String,
    pub layout: Layout,
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Data {
    peers: BTreeMap<String, TrustedPeer>,
    #[serde(default)]
    monitor: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    hide_tray: bool,
    #[serde(default)]
    autostart_prompted: bool,
}
pub struct Store {
    path: PathBuf,
    data: Data,
}
impl Store {
    pub fn load(directory: &Path) -> Result<Self> {
        let path = directory.join("peers.json");
        let data: Data = if path.try_exists()? {
            serde_json::from_slice(&read_private(&path)?)?
        } else {
            Data::default()
        };
        ensure!(data.peers.len() <= 32, "Too many trusted peers");
        if let Some(id) = &data.monitor {
            validate_monitor_id(id)?;
        }
        if let Some(name) = &data.name {
            validate_name(name)?;
        }
        for (fp, peer) in &data.peers {
            validate_fingerprint(fp)?;
            validate_name(&peer.name)?;
            peer.layout.validate()?;
        }
        Ok(Self { path, data })
    }
    pub fn peers(&self) -> BTreeMap<String, TrustedPeer> {
        self.data.peers.clone()
    }
    pub fn monitor(&self) -> Option<&str> {
        self.data.monitor.as_deref()
    }
    pub fn name(&self) -> Option<&str> {
        self.data.name.as_deref()
    }
    pub fn hide_tray(&self) -> bool {
        self.data.hide_tray
    }
    pub fn set_hide_tray(&mut self, hide: bool) -> Result<()> {
        let mut next = self.data.clone();
        next.hide_tray = hide;
        self.save(next)
    }
    pub fn autostart_prompted(&self) -> bool {
        self.data.autostart_prompted
    }
    pub fn set_autostart_prompted(&mut self, prompted: bool) -> Result<()> {
        let mut next = self.data.clone();
        next.autostart_prompted = prompted;
        self.save(next)
    }
    pub fn set_name(&mut self, name: String) -> Result<()> {
        validate_name(&name)?;
        let mut next = self.data.clone();
        next.name = Some(name);
        self.save(next)
    }
    pub fn set_monitor(&mut self, id: String) -> Result<()> {
        validate_monitor_id(&id)?;
        let mut next = self.data.clone();
        next.monitor = Some(id);
        self.save(next)
    }
    pub fn trusted(&self, fp: &str) -> bool {
        self.data.peers.contains_key(fp)
    }
    pub fn layout(&self, fp: &str) -> Layout {
        self.data
            .peers
            .get(fp)
            .map(|p| p.layout)
            .unwrap_or_default()
    }
    pub fn trust(&mut self, fp: String, name: String) -> Result<()> {
        validate_fingerprint(&fp)?;
        validate_name(&name)?;
        ensure!(
            self.data.peers.len() < 32 || self.trusted(&fp),
            "Trust list full; revoke a device first"
        );
        let mut next = self.data.clone();
        next.peers.insert(
            fp,
            TrustedPeer {
                name,
                layout: Layout::default(),
            },
        );
        self.save(next)
    }
    pub fn set_peer_name(&mut self, fp: &str, name: String) -> Result<()> {
        validate_name(&name)?;
        let mut next = self.data.clone();
        let peer = next
            .peers
            .get_mut(fp)
            .ok_or_else(|| anyhow::anyhow!("Peer is not trusted"))?;
        if peer.name == name {
            return Ok(());
        }
        peer.name = name;
        self.save(next)
    }
    pub fn revoke(&mut self, fp: &str) -> Result<()> {
        let mut next = self.data.clone();
        next.peers.remove(fp);
        self.save(next)
    }
    pub fn set_layout(&mut self, fp: &str, layout: Layout) -> Result<()> {
        layout.validate()?;
        let mut next = self.data.clone();
        let peer = next
            .peers
            .get_mut(fp)
            .ok_or_else(|| anyhow::anyhow!("Peer is not trusted"))?;
        peer.layout = layout;
        self.save(next)
    }
    fn save(&mut self, next: Data) -> Result<()> {
        atomic_private(&self.path, &serde_json::to_vec_pretty(&next)?, true)?;
        self.data = next;
        Ok(())
    }
}
fn validate_monitor_id(id: &str) -> Result<()> {
    ensure!(
        !id.is_empty() && id.len() <= 128 && !id.chars().any(char::is_control),
        "Invalid monitor identifier"
    );
    Ok(())
}
pub fn validate_fingerprint(fp: &str) -> Result<()> {
    ensure!(
        fp.len() == 64
            && fp
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
        "Invalid fingerprint"
    );
    Ok(())
}
pub fn validate_name(name: &str) -> Result<()> {
    ensure!(
        !name.is_empty() && name.len() <= 64 && name.chars().all(|c| !c.is_control()),
        "Invalid device name"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn trust_revocation_and_layout_survive_restart() {
        let dir = tempfile::tempdir().unwrap();
        let fp = "ab".repeat(32);
        let mut s = Store::load(dir.path()).unwrap();
        s.trust(fp.clone(), "Laptop".into()).unwrap();
        let layout = Layout {
            transfer: crate::geometry::TransferMode::SendOnly,
            alignment: 0.2,
            ..Layout::default()
        };
        s.set_layout(&fp, layout).unwrap();
        s.set_monitor("DISPLAY2".into()).unwrap();
        s.set_name("My Ravel laptop".into()).unwrap();
        s.set_peer_name(&fp, "Renamed peer".into()).unwrap();
        let mut s = Store::load(dir.path()).unwrap();
        assert_eq!(s.layout(&fp), layout);
        assert_eq!(s.monitor(), Some("DISPLAY2"));
        assert_eq!(s.name(), Some("My Ravel laptop"));
        assert_eq!(s.peers()[&fp].name, "Renamed peer");
        assert!(s.set_name(String::new()).is_err());
        assert!(!s.trusted(&"cd".repeat(32)));
        s.revoke(&fp).unwrap();
        assert!(!Store::load(dir.path()).unwrap().trusted(&fp));
    }

    #[test]
    fn preferences_default_off_and_persist() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = Store::load(dir.path()).unwrap();
        assert!(!s.hide_tray());
        assert!(!s.autostart_prompted());
        s.set_hide_tray(true).unwrap();
        s.set_autostart_prompted(true).unwrap();
        let s = Store::load(dir.path()).unwrap();
        assert!(s.hide_tray());
        assert!(s.autostart_prompted());
    }
}
