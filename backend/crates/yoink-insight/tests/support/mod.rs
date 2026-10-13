use std::{net::IpAddr, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};
use yoink_insight::{provider::HttpCompletion, research::ResponsesResearch};

async fn relay(mut socket: TcpStream) -> std::io::Result<()> {
    let mut greeting = [0; 2];
    socket.read_exact(&mut greeting).await?;
    if greeting[0] != 5 || greeting[1] == 0 || greeting[1] > 8 {
        return Err(std::io::Error::other("invalid SOCKS greeting"));
    }
    let mut methods = vec![0; greeting[1] as usize];
    socket.read_exact(&mut methods).await?;
    if !methods.contains(&0) {
        return Err(std::io::Error::other(
            "fixture requires unauthenticated SOCKS",
        ));
    }
    socket.write_all(&[5, 0]).await?;
    let mut request = [0; 4];
    socket.read_exact(&mut request).await?;
    if request[..3] != [5, 1, 0] {
        return Err(std::io::Error::other("invalid SOCKS request"));
    }
    let ip = match request[3] {
        1 => {
            let mut bytes = [0; 4];
            socket.read_exact(&mut bytes).await?;
            IpAddr::from(bytes)
        }
        4 => {
            let mut bytes = [0; 16];
            socket.read_exact(&mut bytes).await?;
            IpAddr::from(bytes)
        }
        _ => {
            return Err(std::io::Error::other(
                "fixture requires local DNS resolution",
            ));
        }
    };
    let mut port = [0; 2];
    socket.read_exact(&mut port).await?;
    if !ip.is_loopback() {
        return Err(std::io::Error::other(
            "fixture refuses external destinations",
        ));
    }
    let mut target = TcpStream::connect((ip, u16::from_be_bytes(port))).await?;
    socket.write_all(&[5, 0, 0, 1, 127, 0, 0, 1, 0, 0]).await?;
    tokio::io::copy_bidirectional(&mut socket, &mut target).await?;
    Ok(())
}

pub async fn proxy() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("socks5://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        for _ in 0..32 {
            let Ok((socket, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(async move {
                let _ = tokio::time::timeout(Duration::from_secs(10), relay(socket)).await;
            });
        }
    });
    url
}

pub async fn completion(timeout: Duration, bytes: usize) -> HttpCompletion {
    HttpCompletion::new(timeout, bytes)
        .unwrap()
        .with_tunnel(&proxy().await)
        .unwrap()
}

#[allow(dead_code)]
pub async fn research(timeout: Duration, bytes: usize) -> ResponsesResearch {
    ResponsesResearch::new(timeout, bytes)
        .unwrap()
        .with_tunnel(&proxy().await)
        .unwrap()
}
