# Protocol and threat model (version 2)

Ravel is an IPv4 LAN application for two devices. Discovery is an untrusted hint.
Names and addresses never authorize input. No backend, UDP input, plaintext
fallback, clipboard, file transfer or remote shell exists.

## Identity and transport

Each installation owns one persistent ECDSA P-256 self-signed TLS certificate and
private key. The stable identity is SHA-256 of the exact certificate DER; renewal
is an identity change. Linux files are private to the user (directory 0700, files
0600); Windows private identity material uses current-user DPAPI. Trust and layout
are stored per fingerprint. Revocation removes trust and stops the active session.
Corrupt identity files cause an error, never silent identity rotation.

Every socket starts with rustls TLS 1.3, mutual certificates, signature validation,
no 0-RTT and no session resumption. During first pairing certificates are not yet
trusted: TLS is confidential but peer authorization is pending. Application state
permits only a versioned hello and CPace, never input, in this state. For ordinary
connections both sides require a stored fingerprint before entering the session.
The initiating side additionally pins its selected fingerprint when available.

## Pairing

The receiver opens a five-minute offer with a random eight-digit PIN. PINs exist
only in memory. Global offer budget: five admitted exchanges (including abandoned
exchanges), exponential admission delay 1,2,4,8,16 seconds, at most one pairing
exchange in progress. The budget is global rather than per spoofable LAN IP.

Use pinned 1Password CPace with its built-in mutual key confirmation. Channel
identities are the client/server certificate SHA-256 digests. Extra context is
`ravel-pair-v1` and the 32-byte TLS exporter labeled `EXPORTER-Ravel-pair-v1`.
This binds the PAKE to the certificate holders and the particular TLS session,
preventing a TLS-terminating relay from transplanting a successful PIN exchange.
No PIN or derived password verifier crosses the network. A fresh random session
ID is included using the CPace library's SessionAD encoding. No bespoke cipher,
PAKE math or key-confirmation algorithm is implemented in Ravel.

After both CPace tags verify, the offer is consumed. Each user must explicitly
compare the full displayed peer fingerprint against the other machine's local
fingerprint and approve. Approval must occur before the original deadline.
Cancellation, expiry, wrong PIN, concurrent reuse, changed offer, or failure to
persist trust cannot authorize control. Pairing ends the socket; control requires
a new mutually trusted TLS session. Partial approvals may leave one-sided trust;
re-pair explicitly on both devices. Revoked identities cannot reconnect without a
new PIN and explicit approval. A changed fingerprint is a different device; it
must never inherit trust by name or address.

CPace's implementation audit is not a review of this application composition.
An independent composition/state-machine review is a release gate.

## Frames and control

TLS contains a big-endian u32 byte length and JSON enum, maximum 4096 bytes.
Length is checked before allocating. Protocol version must match exactly; no
downgrade. Hello names: at most 64 printable characters. Finite pointer coordinates
and scroll deltas are bounded; scancodes, buttons, states, and modifier masks are
allowlisted. Unknown enums/fields and invalid transitions fail the connection.
Only one control session is admitted. Both sides send the selected handoff
monitor and full desktop geometry before arming capture. Motion coordinates use
the full desktop; only the selected monitor's outer edge transfers control.
Each input stream needs an Enter before Input and Leave before
another Enter. Receiving disables capture to prevent echo. All keys, modifiers
and mouse buttons are released on leave, error, stop, timeout and normal exit.

Heartbeats every 1 second, inbound inactivity timeout 4 seconds. Read/write and
handshake operations have deadlines. The complete display exchange (write and
read) has a five-second deadline and is interruptible by stop before capture is
armed. Trust and stop state are checked again after that exchange. Input queues are bounded; congestion closes
the session rather than dropping key-up events. Ctrl+Alt+Shift+Esc stops sharing
while capture is active; the GUI always has a stop/disconnect button. Input and
secret payloads are never logged. The OS can still revoke grabs/permissions or
terminate the process; abrupt process death cannot guarantee cleanup.

## Network changes and abuse

mDNS/DNS-SD `_ravel._tcp.local.` advertises version, untrusted name, fingerprint
hint and current port. Refresh interfaces every two seconds, auto-update addresses,
and remove vanished services. Only private/link-local IPv4 peers/endpoints are
accepted; loopback is reserved for tests. Public Internet/IPv6 are not supported.
No UPnP or firewall changes are performed. Manual host:port resolves dynamically
and is subject to the same LAN restrictions and authentication gates.

An attacker can spoof discovery, flood sockets or consume a pairing offer's limited
guesses (denial of service), but cannot trigger injection without trust and an
active authenticated session. Bound concurrent handshakes and UI/event queues.
A compromised paired device can inject input intentionally; revoke it immediately.
Local malware and users with access to the account are outside the protection
boundary. OS elevation/secure desktops are unsupported.
