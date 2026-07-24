use crate::auth::{AuthInput, AuthState};
use crate::{BatteryStatus, Capabilities, Command, DeviceInfo, Error, Result, StatusUpdate};
use async_trait::async_trait;
use core::pin::Pin;
use futures_core::Stream;

/// A borrowed, real-time stream of incremental [`StatusUpdate`]s produced by a
/// backend that can *push* telemetry (e.g. BLE notifications). Each item is a
/// single change (or a transport error). The stream borrows the device for its
/// lifetime, so commands are issued between updates, not concurrently.
pub type StatusStream<'a> = Pin<Box<dyn Stream<Item = Result<StatusUpdate>> + Send + 'a>>;

/// A uniform, async interface to any supported battery / BMS / power station.
///
/// Implementors are thin adapters over a device-specific crate or protocol.
/// Read support is expressed by populating [`BatteryStatus`]; control support is
/// gated by [`Capabilities`] and dispatched through [`Command`].
#[async_trait]
pub trait Battery: Send {
    /// Static device identity.
    fn info(&self) -> &DeviceInfo;

    /// What this device can read and control.
    fn capabilities(&self) -> Capabilities;

    /// Fetch a fresh, normalized status snapshot.
    async fn status(&mut self) -> Result<BatteryStatus>;

    /// Execute a control command.
    ///
    /// The default implementation rejects everything with [`Error::Unsupported`];
    /// controllable backends override this. Implementations should return
    /// [`Error::Unsupported`] for any individual command they don't handle.
    async fn execute(&mut self, cmd: Command) -> Result<()> {
        let _ = cmd;
        Err(Error::Unsupported)
    }

    /// Drive the authentication / binding flow for devices that require pairing
    /// (capability [`Capabilities::REQUIRES_AUTH`]).
    ///
    /// Call repeatedly until [`AuthState::Authed`]: pass [`AuthInput::None`] to
    /// start (or to retry after the user has performed a physical approval), or
    /// [`AuthInput::Pin`] when the previous step returned [`AuthState::PinCode`].
    /// Backends persist any resulting credential (see [`crate::credentials`]) so
    /// later connects return `Authed` immediately.
    ///
    /// The default is a no-op `Authed` for devices that need no auth.
    async fn authenticate(&mut self, input: AuthInput) -> Result<AuthState> {
        let _ = input;
        Ok(AuthState::Authed)
    }

    /// Forget any saved pairing/credential for this device, so the next
    /// [`authenticate`](Self::authenticate) starts the flow fresh. Note this
    /// clears the *local* record only; a device-side bond (if any) persists
    /// until reset on the device. Default is a no-op.
    async fn forget_auth(&mut self) -> Result<()> {
        Ok(())
    }

    /// A **real-time** stream of incremental [`StatusUpdate`]s, for backends
    /// that push updates over their transport (e.g. Anker SOLIX BLE
    /// notifications). Each item is a single field/port/cell change delivered
    /// the moment the device reports it — no polling interval, no full-snapshot
    /// churn. Reconstruct state by starting from [`status`](Self::status) (or
    /// `BatteryStatus::default`) and applying updates.
    ///
    /// Returns `None` for pull-only backends (serial round-trip, CAN request);
    /// callers should fall back to periodically invoking [`status`](Self::status)
    /// — or just use [`updates`](Self::updates), which does this for you.
    fn stream(&mut self) -> Option<StatusStream<'_>> {
        None
    }

    /// Whether [`stream`](Self::stream) returns a native push stream.
    ///
    /// Backends that override `stream` **must** also override this to return
    /// `true`; it lets [`updates`](Self::updates) pick the native path without
    /// consuming the `&mut self` borrow probing for it.
    fn has_stream(&self) -> bool {
        false
    }

    /// **Unified** real-time updates for any backend — the recommended way to
    /// consume live state.
    ///
    /// Uses the native push [`stream`](Self::stream) when the backend has one
    /// (no polling, updates arrive as the device emits them); otherwise polls
    /// [`status`](Self::status) every `poll` and diffs. Either way the caller
    /// sees the same thing: the first items describe the full current state
    /// (diff from empty), then only changes. Maintain a live snapshot by
    /// applying each update with [`BatteryStatus::apply`]. On error the stream
    /// yields it once and ends, so callers can reconnect instead of spinning.
    #[cfg(feature = "runtime")]
    fn updates(&mut self, poll: core::time::Duration) -> StatusStream<'_> {
        if self.has_stream() {
            // Unconditional return keeps the borrow from escaping into the
            // polling path below (NLL can't see the arms are exclusive).
            return self
                .stream()
                .expect("has_stream() returned true but stream() returned None");
        }
        let init = (self, None::<BatteryStatus>, std::collections::VecDeque::new(), false);
        Box::pin(futures_util::stream::unfold(
            init,
            move |(this, mut prev, mut queue, ended)| async move {
                loop {
                    if let Some(u) = queue.pop_front() {
                        return Some((Ok(u), (this, prev, queue, ended)));
                    }
                    if ended {
                        return None;
                    }
                    if prev.is_some() {
                        tokio::time::sleep(poll).await;
                    }
                    match this.status().await {
                        Ok(s) => {
                            queue.extend(s.diff(prev.as_ref()));
                            prev = Some(s);
                        }
                        Err(e) => {
                            return Some((Err(e), (this, prev, queue, true)));
                        }
                    }
                }
            },
        ))
    }

    /// Cleanly close the underlying transport (e.g. send a BLE disconnect).
    ///
    /// Dropping an adapter frees memory but does **not** necessarily tear down
    /// a BLE link — CoreBluetooth/BlueZ keep the peripheral connected until an
    /// explicit disconnect. Call this before dropping to release the radio. The
    /// default is a no-op (serial/CAN close on drop).
    async fn disconnect(&mut self) -> Result<()> {
        Ok(())
    }
}

/// Helper for adapters: assert a capability is present before acting on a command.
pub(crate) fn require(caps: Capabilities, needed: Capabilities) -> Result<()> {
    if caps.contains(needed) {
        Ok(())
    } else {
        Err(Error::Unsupported)
    }
}
