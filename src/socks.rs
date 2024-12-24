use core::str;
use std::io::Write;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;

use anyhow::Result;
use byteorder::WriteBytesExt;
use log::debug;
use tokio::io::AsyncReadExt;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream};

use crate::pipe_srv::Pipe;
use crate::pipe_wire;

#[derive(Debug)]
struct SockMessageClientAuth {
    methods: Vec<u8>,
}

#[derive(Debug)]
struct SockMessageConnect {
    address: String,
    port: u16,
}

#[derive(Debug)]
enum SockMessage {
    Connect(SockMessageConnect),
}

async fn read_socks_auth(socket: &mut TcpStream) -> Result<SockMessageClientAuth> {
    let mut header: [u8; 2] = [0, 0];
    socket.read_exact(&mut header).await?;
    let mut methods = vec![0; header[1] as usize];
    socket.read_exact(methods.as_mut_slice()).await?;
    Ok(SockMessageClientAuth { methods })
}

async fn read_socks_connect(socket: &mut TcpStream) -> Result<SockMessageConnect> {
    socket.read_u8().await?;
    let address_type = socket.read_u8().await?;
    let address = match address_type {
        1 => {
            let mut buf = vec![0; 4];
            socket.read_exact(&mut buf).await?;
            format!("{}.{}.{}.{}", buf[0], buf[1], buf[2], buf[3])
        }
        3 => {
            let addr_len = socket.read_u8().await?;
            let mut buf = vec![0; addr_len as usize];
            socket.read_exact(&mut buf).await?;
            str::from_utf8(&buf)?.to_owned()
        }
        at => return Err(anyhow::anyhow!("unknown address type {}", at)),
    };
    let port = socket.read_u16().await?;
    debug!("addr:{} port: {}", address, port);
    return Ok(SockMessageConnect {
        address,
        port,
    });
}

async fn read_socks_message(socket: &mut TcpStream) -> Result<SockMessage> {
    let mut header: [u8; 2] = [0, 0];
    socket.read_exact(&mut header).await?;
    if header[1] == 1 {
        return Ok(SockMessage::Connect(read_socks_connect(socket).await?));
    }
    Err(anyhow::anyhow!("unknown command"))
}

fn socks5_connect_response(status: u8) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(64);
    out.write_u8(5)?;
    out.write_u8(status)?;
    out.write_u8(0)?; // reserved
    out.write_u8(1)?; // address type - ipv4
    out.write_all(&[0,0,0,0])?; // address type - ipv4
    out.write_all(&[0,0])?; // port
    Ok(out)
}

async fn handle_messages_from_pipe(mut pipe_incoming_receiver: tokio::sync::mpsc::Receiver<Vec<u8>>, mut client_writer: OwnedWriteHalf) {
    loop {
        let from_pipe = match pipe_incoming_receiver.recv().await {
            Some(f) => f,
            None => break,
        };
        if from_pipe.len() == 0 {
            break;
        }
        match from_pipe[0] {
            pipe_wire::CMD_CLOSE => {
                debug!("close from pipe");
                break;
            }
            pipe_wire::CMD_DATA => {
                let data = &from_pipe[1..];
                if tokio::io::AsyncWriteExt::write_all(&mut client_writer, data).await.is_err() {
                    break
                }
            }
            cmd => {
                debug!("unknown msg from pipe {}", cmd)
            }
        }
    }
    debug!("reader task break");
}

async fn handle_data_from_client(mut client_reader: OwnedReadHalf, pipe: Arc<Pipe>, id: u64) -> Result<()>{
    let mut buf = vec![0; 1024 * 32];
    loop {
        let n = client_reader.read(&mut buf).await?;
        if n == 0 {
            return Ok(())
        }
        let datamsg = pipe_wire::encode_data(id, &buf[..n])?;
        pipe.send(datamsg).unwrap();
    }
}


async fn connection_loop(pipe: Arc<Pipe>, mut socket: TcpStream, id: u64) -> Result<()> {
    let auth_methods = read_socks_auth(&mut socket).await?;
    debug!("socks auth methods received: {:?}", auth_methods.methods);
    let auth_resp: [u8; 2] = [5, 0];
    tokio::io::AsyncWriteExt::write_all(&mut socket, &auth_resp).await?;
    let cmd = read_socks_message(&mut socket).await?;
    debug!("got cmd {:?}", cmd);
    if let SockMessage::Connect(connect_msg) = cmd {
        let (pipe_incoming_writer, mut pipe_incoming_receiver) = tokio::sync::mpsc::channel(1024);
        let pipe_incoming_writer_c = pipe_incoming_writer.clone();

        pipe.connect(
            id,
            &connect_msg.address,
            connect_msg.port,
            pipe_incoming_writer,
        )
        .await?;

        let (client_reader, mut client_writer) = socket.into_split();

        let from_pipe = match pipe_incoming_receiver.recv().await {
            Some(f) => f,
            None => {
                tokio::io::AsyncWriteExt::write_all(&mut client_writer, &socks5_connect_response(1)?).await?;
                pipe.close(id).await.unwrap();
                return Err(anyhow::anyhow!("error receiving connect response from pipe"))
            },
        };
        if (from_pipe.len() != 2) || (from_pipe[0] != pipe_wire::CMD_CONNECT_ACK) {
            tokio::io::AsyncWriteExt::write_all(&mut client_writer, &socks5_connect_response(1)?).await?;
            pipe.close(id).await.unwrap();
            return Err(anyhow::anyhow!("error receiving connect response from pipe"))
        }
        if from_pipe[1] != pipe_wire::CONNECT_STATUS_OK {
            tokio::io::AsyncWriteExt::write_all(&mut client_writer, &socks5_connect_response(5)?).await?;
            pipe.close(id).await.unwrap();
            return Err(anyhow::anyhow!("connect response is not ok {}", from_pipe[1]))
        }

        let connect_resp = socks5_connect_response(0)?;
        tokio::io::AsyncWriteExt::write_all(&mut client_writer, &connect_resp).await?;

        tokio::spawn(async move {
            handle_messages_from_pipe(pipe_incoming_receiver, client_writer).await;
        });

        let _ = handle_data_from_client(client_reader, pipe.clone(), id).await;

        pipe.close(id).await.unwrap();
        let _ = pipe_incoming_writer_c.send(Vec::new()).await;
        debug!("connection break");
        Ok(())
    } else {
        Err(anyhow::anyhow!("unsupported command"))
    }
}


async fn server_loop(pipe: Arc<Pipe>, listen_address: &str) -> Result<()> {
    let listener = TcpListener::bind(&listen_address).await?;
    let id_counter = AtomicU64::new(1);
    loop {
        let (socket, _remote_addr) = listener.accept().await?;
        let p = pipe.clone();
        let id = id_counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        tokio::spawn(async move {
            if let Err(e) = connection_loop(p, socket, id).await {
                debug!("connection loop ended with error {}", e.to_string())
            }
        });
    }
}

pub fn start_socks_server(pipe_listen_addr: &str, socks_listen_addr: &str) {
    let runtime = tokio::runtime::Runtime::new().unwrap();

    runtime.block_on(async {
        let pipe = Pipe::start(pipe_listen_addr).await;
        server_loop(pipe, socks_listen_addr).await.unwrap();
    });
}
