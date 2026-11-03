use super::{Request, Updater};

pub async fn transfer(
    context: &mbedtls_rs::Tls<'static>,
    socket: embassy_net::tcp::TcpSocket<'_>,
    ota: &mut Updater<'_>,
    request: &Request,
) -> Result<(), &'static str> {
    let mut stream =
        crate::tls::connect(context, crate::tls::Socket(socket), &request.host).await?;
    super::transfer(&mut stream, ota, request).await
}
