use super::*;
use core::cell::Cell;
use embassy_futures::select::{Either, select};
use embassy_sync::{blocking_mutex::raw::NoopRawMutex, pipe::Pipe, signal::Signal};
use embedded_io_async::{ErrorType, Read, Write};

struct Stream<'a, R> {
    reader: R,
    tx: &'a Pipe<NoopRawMutex, 2048>,
    sent: usize,
    drained: &'a Cell<usize>,
    progress: &'a Signal<NoopRawMutex, ()>,
}
impl<R: Read> ErrorType for Stream<'_, R> {
    type Error = R::Error;
}
impl<R: Read> Read for Stream<'_, R> {
    async fn read(&mut self, bytes: &mut [u8]) -> Result<usize, Self::Error> {
        self.reader.read(bytes).await
    }
}
impl<R: Read> Write for Stream<'_, R> {
    async fn write(&mut self, bytes: &[u8]) -> Result<usize, Self::Error> {
        if bytes.is_empty() {
            return Ok(0);
        }
        let n = self.tx.write(bytes).await;
        self.sent = self.sent.wrapping_add(n);
        Ok(n)
    }
    async fn flush(&mut self) -> Result<(), Self::Error> {
        while self.drained.get() != self.sent {
            self.progress.wait().await;
        }
        Ok(())
    }
}

pub(super) async fn run(
    tls: &mut mbedtls_rs::Session<'_, crate::tls::Socket<'_>>,
    identity: &DeviceIdentity,
    settings: &esp32_led_mqtt::device_config::DeviceConfig,
    events: &mut Events,
) -> Result<(), MqttRunError> {
    let tx = Pipe::<NoopRawMutex, 2048>::new();
    let drained = Cell::new(0_usize);
    let progress = Signal::<NoopRawMutex, ()>::new();
    let (reader, mut writer) = tls.split().await.map_err(MqttRunError::TlsIo)?;
    // recv() may send keepalives before select cancels it. Keep TLS writes in
    // this pump so cancellation cannot replay an unacknowledged partial record.
    let outgoing = async {
        let mut bytes = [0; 1024];
        let result: Result<(), MqttRunError> = loop {
            let n = tx.read(&mut bytes).await;
            if let Err(err) = writer.write_all(&bytes[..n]).await {
                break Err(MqttRunError::TlsIo(err));
            }
            if let Err(err) = writer.flush().await {
                break Err(MqttRunError::TlsIo(err));
            }
            drained.set(drained.get().wrapping_add(n));
            progress.signal(());
        };
        result
    };
    let stream = Stream {
        reader,
        tx: &tx,
        sent: 0,
        drained: &drained,
        progress: &progress,
    };
    match select(
        super::run_connected(stream, identity, settings, events),
        outgoing,
    )
    .await
    {
        Either::First(result) => result,
        Either::Second(result) => result,
    }
}
