use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use crate::pipe_wire;
use std::net::ToSocketAddrs;
use log::debug;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::OwnedReadHalf;
use tokio::net::TcpStream;
use anyhow::Result;
use tokio::sync::RwLock;

struct Connection {
    remote_sender: tokio::sync::mpsc::Sender<Vec<u8>>,
}

pub async fn handle_connection_logic(
    cfg: pipe_wire::Connect,
    id: u64,
    pipe_sender: tokio::sync::mpsc::Sender<Vec<u8>>,
    mut client_reader: tokio::sync::mpsc::Receiver<Vec<u8>>,
) -> Result<()>{
        let mut remote_address = match (cfg.host.clone(), cfg.port).to_socket_addrs() {
            Ok(o) => o,
            Err(e) => {
                debug!("can't resolve address {} {}", cfg.host, e.to_string());
                pipe_sender.send(pipe_wire::encode_connect_ack(id, pipe_wire::CONNECT_STATUS_ERR)?).await?;
                return Err(anyhow::anyhow!(e.to_string()))
            }
        };
        let remote_addr = remote_address.next().unwrap();
        debug!("resolved");

        let socket = match TcpStream::connect(remote_addr).await {
            Ok(s) => s,
            Err(e) => {
                // send error to adapter
                pipe_sender.send(pipe_wire::encode_connect_ack(id, pipe_wire::CONNECT_STATUS_ERR)?).await?;
                return Err(anyhow::anyhow!(e.to_string()))
            }
        };
        pipe_sender.send(pipe_wire::encode_connect_ack(id, pipe_wire::CONNECT_STATUS_OK)?).await?;
        let (mut remote_socket_reader, mut remote_socket_writer) = socket.into_split();
        tokio::spawn(async move {
            loop {
                let to_send = match client_reader.recv().await {
                    Some(s) => s,
                    None => break,
                };
                if to_send.len() == 0 {
                    break;
                }
                if let Err(e) = remote_socket_writer.write_all(&to_send).await {
                    debug!("can't write to remote writer {}", e.to_string());
                    break;
                }
            }
            debug!("writer to remote done");
        });
        loop {
            let mut buf = vec![0; 1024 * 32];
            let n = match remote_socket_reader.read(buf.as_mut_slice()).await {
                Ok(n) => n,
                Err(_) => break,
            };
            if n == 0 {
                break;
            }
            let encap = pipe_wire::encode_data(id, &buf[..n])?;
            pipe_sender.send(encap).await?;
        }
        let closemsg = pipe_wire::encode_close(id)?;
        pipe_sender.send(closemsg).await?;
        debug!("reader from remote done");
        Ok(())
}

pub async fn start_handle_connection(
    cfg: pipe_wire::Connect,
    id: u64,
    pipe_sender: tokio::sync::mpsc::Sender<Vec<u8>>,
    client_reader: tokio::sync::mpsc::Receiver<Vec<u8>>,
) {
    tokio::spawn(async move {
        handle_connection_logic(cfg, id, pipe_sender, client_reader).await
    });
}

async fn handle_data_from_pipe(mut s_reader: OwnedReadHalf,
    pipe_sender_writer: tokio::sync::mpsc::Sender<Vec<u8>>,
    connections: Arc<RwLock<HashMap<u64, Connection>>>
) -> Result<()> {
    loop {
        let mut header_buf = vec![0_u8; 12];
        s_reader.read_exact(header_buf.as_mut_slice()).await?;
        let header = pipe_wire::decode_header(&header_buf).unwrap();
        debug!("received some {:?}", header);
        let mut buf = vec![0_u8; header.size as usize];
        s_reader.read_exact(buf.as_mut_slice()).await.unwrap();
        if buf[0] == pipe_wire::CMD_CONNECT {
            let connect = pipe_wire::decode_connect(&buf[1..]).unwrap();
            debug!("received connect {:?}", connect);
            let (client_sender_writer, client_sender_reader) =
                tokio::sync::mpsc::channel::<Vec<u8>>(1024);
            let mut lcon = connections.write().await;
            lcon.insert(
                header.id,
                Connection {
                    remote_sender: client_sender_writer,
                },
            );
            start_handle_connection(
                connect,
                header.id,
                pipe_sender_writer.clone(),
                client_sender_reader,
            )
            .await;
        }
        if buf[0] == pipe_wire::CMD_DATA {
            let datamsg = pipe_wire::decode_data(&buf[1..]).unwrap();
            let lcon = connections.read().await;
            if let Some(c) = lcon.get(&header.id) {
                if let Err(e) = c.remote_sender.send(datamsg).await {
                    debug!("can send msg to remote {} {}", header.id, e.to_string());
                }
            }
        }
        if buf[0] == pipe_wire::CMD_CLOSE {
            debug!("received close");
            let mut lcon = connections.write().await;
            if let Some(c) = lcon.get(&header.id) {
                if let Err(e) = c.remote_sender.send(Vec::new()).await {
                    debug!("cant deliver close to sender {}", e.to_string());
                }
            }
            lcon.remove(&header.id);
            debug!("active connections {}", lcon.len())
        }
    }
}

pub fn gw_start(socks_pipe_addr: &str) {

    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        let connections = Arc::new(tokio::sync::RwLock::new(HashMap::<u64, Connection>::new()));
        loop {
            let socket = match TcpStream::connect(socks_pipe_addr).await {
                Ok(s) => s,
                Err(e) => {
                    debug!("{:?}", e);
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    continue;
                }
            };
            let (s_reader, mut s_writer) = socket.into_split();
            let (pipe_sender_writer, mut pipe_sender_reader) =
                tokio::sync::mpsc::channel::<Vec<u8>>(1024);
            tokio::task::spawn(async move {
                loop {
                    match pipe_sender_reader.recv().await {
                        Some(to_send) => {
                            if s_writer.write_all(&to_send).await.is_err() {
                                break
                            }
                        },
                        None => break,
                    }
                }
            });
            if let Err(e) = handle_data_from_pipe(s_reader, pipe_sender_writer, connections.clone()).await {
                debug!("data_from_pipe handler ended with {}", e.to_string())
            }
        }
    });
}
