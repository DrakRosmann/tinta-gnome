//! Background threads talk to the app through one channel, which the GTK main loop listens to.

use crate::app::Message;
use std::sync::OnceLock;

struct Bus {
    tx: async_channel::Sender<Message>,
    rx: async_channel::Receiver<Message>,
}

fn bus() -> &'static Bus {
    static BUS: OnceLock<Bus> = OnceLock::new();
    BUS.get_or_init(|| {
        let (tx, rx) = async_channel::unbounded();
        Bus { tx, rx }
    })
}

pub fn send(msg: Message) {
    let _ = bus().tx.try_send(msg);
}

/// The receiving end, for the main loop.
pub fn receiver() -> async_channel::Receiver<Message> {
    bus().rx.clone()
}

/// Tests have no main loop: they empty the channel themselves.
#[cfg(test)]
pub fn drain() -> Vec<Message> {
    let mut out = Vec::new();
    while let Ok(m) = bus().rx.try_recv() {
        out.push(m);
    }
    out
}
