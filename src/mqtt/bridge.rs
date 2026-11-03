use super::*;
use core::sync::atomic::{AtomicUsize, Ordering};
use embassy_futures::select::{Either, select};
use embassy_sync::{blocking_mutex::raw::NoopRawMutex, pipe::Pipe, signal::Signal};
use embedded_io_async::{ErrorKind, ErrorType, Read, Write};

struct Stream<'a> {
    rx: &'a Pipe<NoopRawMutex, 2048>,
    tx: &'a Pipe<NoopRawMutex, 2048>,
    sent: usize,
    drained: &'a AtomicUsize,
    progress: &'a Signal<NoopRawMutex, ()>,
}
impl ErrorType for Stream<'_> {
    type Error = ErrorKind;
}
impl Read for Stream<'_> {
    async fn read(&mut self, bytes: &mut [u8]) -> Result<usize, Self::Error> {
        if bytes.is_empty() {
            return Ok(0);
        }
        Ok(self.rx.read(bytes).await)
    }
}
impl Write for Stream<'_> {
    async fn write(&mut self, bytes: &[u8]) -> Result<usize, Self::Error> {
        if bytes.is_empty() {
            return Ok(0);
        }
        let n = self.tx.write(bytes).await;
        self.sent = self.sent.wrapping_add(n);
        Ok(n)
    }
    async fn flush(&mut self) -> Result<(), Self::Error> {
        while self.drained.load(Ordering::Acquire) != self.sent {
            self.progress.wait().await;
        }
        Ok(())
    }
}

pub(super) async fn run(
    tls: &mut mbedtls_rs::Session<'_, crate::tls::Socket<'_>>,
    identity: &DeviceIdentity,
    settings: &esp32_led_mqtt::device_config::DeviceConfig,
) -> Result<(), MqttRunError> {
    let rx = Pipe::<NoopRawMutex, 2048>::new();
    let tx = Pipe::<NoopRawMutex, 2048>::new();
    let drained = AtomicUsize::new(0);
    let progress = Signal::<NoopRawMutex, ()>::new();
    let (mut reader, mut writer) = tls.split().await.map_err(|_| MqttRunError::Transport)?;
    // Only termination of the entire connection cancels these TLS operations.
    let incoming = async {
        let mut bytes = [0; 1024];
        loop {
            let n = reader.read(&mut bytes).await.map_err(|_| ())?;
            if n == 0 {
                return Err::<(), ()>(());
            }
            let mut offset = 0;
            while offset < n {
                offset += rx.write(&bytes[offset..n]).await;
            }
        }
    };
    let outgoing = async {
        let mut bytes = [0; 1024];
        loop {
            let n = tx.read(&mut bytes).await;
            writer.write_all(&bytes[..n]).await.map_err(|_| ())?;
            writer.flush().await.map_err(|_| ())?;
            drained.fetch_add(n, Ordering::Release);
            progress.signal(());
        }
        #[allow(unreachable_code)]
        Ok::<(), ()>(())
    };
    let pump = async {
        select(incoming, outgoing).await;
    };
    let stream = Stream {
        rx: &rx,
        tx: &tx,
        sent: 0,
        drained: &drained,
        progress: &progress,
    };
    match select(super::run_connected(stream, identity, settings), pump).await {
        Either::First(result) => result,
        Either::Second(()) => Err(MqttRunError::Transport),
    }
}
