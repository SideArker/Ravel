use crate::protocol::{self, Message};
use anyhow::{Result, anyhow, ensure};
use pake_cpace::{CPace, ChannelIdentifier, NoAD, SessionAD};
use rand::{Rng, RngCore, rngs::OsRng};
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncWrite};
use zeroize::{Zeroize, Zeroizing};

pub const PIN_LIFETIME: Duration = Duration::from_secs(300);
const ATTEMPTS: u8 = 5;
pub struct Offer {
    pin: Zeroizing<String>,
    pub deadline: Instant,
    generation: u64,
    attempts: u8,
    next_attempt: Instant,
    busy: bool,
    consumed: bool,
}
pub struct Lease {
    pub pin: Zeroizing<String>,
    pub generation: u64,
    pub deadline: Instant,
}
impl Offer {
    pub fn new(now: Instant) -> Self {
        Self {
            pin: Zeroizing::new(format!("{:08}", OsRng.gen_range(0..100_000_000u32))),
            deadline: now + PIN_LIFETIME,
            generation: OsRng.next_u64(),
            attempts: 0,
            next_attempt: now,
            busy: false,
            consumed: false,
        }
    }
    pub fn display_pin(&self) -> String {
        self.pin.to_string()
    }
    pub fn admit(&mut self, now: Instant) -> Result<Lease> {
        ensure!(
            !self.consumed && now < self.deadline,
            "Pairing PIN expired or already used"
        );
        ensure!(
            self.attempts < ATTEMPTS,
            "Pairing attempt limit reached; create a new PIN locally"
        );
        ensure!(!self.busy && now >= self.next_attempt, "Pairing throttled");
        self.busy = true;
        self.next_attempt = now + Duration::from_secs(1 << self.attempts);
        self.attempts += 1;
        Ok(Lease {
            pin: Zeroizing::new(self.pin.to_string()),
            generation: self.generation,
            deadline: self.deadline,
        })
    }
    pub fn finish(&mut self, generation: u64, success: bool, now: Instant) -> Result<()> {
        ensure!(
            generation == self.generation && self.busy,
            "Pairing offer changed"
        );
        self.busy = false;
        ensure!(
            !self.consumed && now < self.deadline,
            "Pairing PIN expired or already used"
        );
        if success {
            self.consumed = true;
            self.pin.zeroize();
        }
        Ok(())
    }
}

pub fn validate_pin(pin: &str) -> Result<()> {
    ensure!(
        pin.len() == 8 && pin.bytes().all(|b| b.is_ascii_digit()),
        "Enter the eight-digit PIN shown on the other device"
    );
    Ok(())
}

pub struct Binding<'a> {
    pub client_fingerprint: &'a str,
    pub server_fingerprint: &'a str,
    pub exporter: &'a [u8; 32],
}
impl Binding<'_> {
    fn channel<'a>(&'a self, context: &'a [&'a [u8]]) -> ChannelIdentifier<'a> {
        ChannelIdentifier {
            id_a: self.client_fingerprint.as_bytes(),
            id_b: self.server_fingerprint.as_bytes(),
            extra_context: context,
        }
    }
}
fn pake_error(e: impl std::fmt::Display) -> anyhow::Error {
    anyhow!("Pairing authentication failed: {e}")
}

pub async fn exchange<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    pin: &str,
    binding: Binding<'_>,
    initiator: bool,
) -> Result<()> {
    validate_pin(pin)?;
    let extra: &[&[u8]] = &[b"ravel-pair-v1", binding.exporter];
    let channel = binding.channel(extra);
    let negotiation = if initiator {
        let mut session_id = [0u8; 16];
        OsRng.fill_bytes(&mut session_id);
        let ctx = CPace::new(
            &session_id,
            pin.as_bytes(),
            &channel,
            &SessionAD { session_id },
        )
        .map_err(pake_error)?;
        protocol::write(stream, &Message::Pake(ctx.message().to_vec())).await?;
        let Message::Pake(message) = protocol::read(stream).await? else {
            return Err(anyhow!("Expected CPace response"));
        };
        ensure!(message.len() == 32, "Invalid CPace response length");
        ctx.initiator_process_message::<NoAD>(message)
            .map_err(pake_error)?
            .0
    } else {
        let Message::Pake(message) = protocol::read(stream).await? else {
            return Err(anyhow!("Expected CPace request"));
        };
        ensure!(message.len() == 48, "Invalid CPace request length");
        let (processed, ad) =
            CPace::responder_process_message::<SessionAD>(message).map_err(pake_error)?;
        let ctx =
            CPace::new(&ad.session_id, pin.as_bytes(), &channel, &NoAD {}).map_err(pake_error)?;
        let negotiation = processed.finish(ctx).map_err(pake_error)?;
        protocol::write(stream, &Message::Pake(negotiation.own_message().to_vec())).await?;
        negotiation
    };
    // The audited library implements both confirmation tags; do not duplicate it.
    protocol::write(stream, &Message::Tag(negotiation.tag().to_vec())).await?;
    let Message::Tag(tag) = protocol::read(stream).await? else {
        return Err(anyhow!("Expected CPace confirmation"));
    };
    ensure!(tag.len() == 64, "Invalid CPace confirmation length");
    let _key = Zeroizing::new(negotiation.verify_tag(&tag).map_err(pake_error)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn expires_single_use_limits_and_backoff() {
        let now = Instant::now();
        let mut o = Offer::new(now);
        assert_eq!(o.deadline - now, PIN_LIFETIME);
        let lease = o.admit(now).unwrap();
        assert!(o.admit(now + Duration::from_secs(2)).is_err());
        o.finish(lease.generation, false, now).unwrap();
        assert!(o.admit(now).is_err());
        for i in 1..5 {
            let t = now + Duration::from_secs(1 << i);
            let l = o.admit(t).unwrap();
            o.finish(l.generation, false, t).unwrap();
        }
        assert!(o.admit(now + Duration::from_secs(100)).is_err());
        let mut o = Offer::new(now);
        let l = o.admit(now).unwrap();
        o.finish(l.generation, true, now).unwrap();
        assert!(o.admit(now + Duration::from_secs(10)).is_err());
        let mut o = Offer::new(now);
        assert!(o.admit(now + PIN_LIFETIME).is_err());
        let mut o = Offer::new(now);
        let l = o.admit(now).unwrap();
        assert!(o.finish(l.generation, true, now + PIN_LIFETIME).is_err());
    }
    async fn run(
        pin_a: &str,
        pin_b: &str,
        exporter_b: [u8; 32],
        server_fp_b: &str,
    ) -> (bool, bool) {
        let (mut a, mut b) = tokio::io::duplex(8192);
        let x = [1u8; 32];
        let a = exchange(
            &mut a,
            pin_a,
            Binding {
                client_fingerprint: "alice",
                server_fingerprint: "bob",
                exporter: &x,
            },
            true,
        );
        let b = exchange(
            &mut b,
            pin_b,
            Binding {
                client_fingerprint: "alice",
                server_fingerprint: server_fp_b,
                exporter: &exporter_b,
            },
            false,
        );
        let (a, b) = tokio::join!(a, b);
        (a.is_ok(), b.is_ok())
    }
    #[tokio::test]
    async fn pin_identity_and_tls_binding_are_all_required() {
        assert_eq!(
            run("12345678", "12345678", [1; 32], "bob").await,
            (true, true)
        );
        assert_eq!(
            run("12345678", "87654321", [1; 32], "bob").await,
            (false, false)
        );
        assert_eq!(
            run("12345678", "12345678", [2; 32], "bob").await,
            (false, false)
        );
        assert_eq!(
            run("12345678", "12345678", [1; 32], "mallory").await,
            (false, false)
        );
    }
}
