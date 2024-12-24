use std::{collections::HashMap, sync::Arc};

use crate::pipe_wire;
use anyhow::Result;
use log::debug;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{tcp::OwnedReadHalf, TcpListener},
    sync::mpsc::UnboundedSender,
};

pub struct Pipe {
    connections: Arc<tokio::sync::RwLock<HashMap<u64, Connection>>>,
    sender: UnboundedSender<Vec<u8>>,
}

struct Connection {
    id: u64,
    incoming_writer: tokio::sync::mpsc::Sender<Vec<u8>>,
}

impl Pipe {
    async fn handle_data_from_pipe(&self, socket_reader: &mut OwnedReadHalf) -> Result<()>{
        loop {
            let mut header_buf = vec![0_u8; 12];
            if socket_reader
                .read_exact(header_buf.as_mut_slice())
                .await
                .is_err()
            {
                // pipe error - go out
                break;
            }
            let header = pipe_wire::decode_header(&header_buf)?;
            //debug!("got header {:?}", header);
            let mut buf = vec![0_u8; header.size as usize];
            socket_reader.read_exact(buf.as_mut_slice()).await?;
            let clock = self.connections.read().await;
            let c = clock.get(&header.id);
            match c {
                Some(c) => {
                    //println!("{:?}", buf);
                    match c.incoming_writer.send(buf).await {
                        Ok(_) => {}
                        Err(e) => {
                            debug!("can't send data to incoming writer from pipe {}", e.to_string())
                        }
                    }
                }
                None => {
                    debug!("unknown connection {}", header.id);
                    continue;
                }
            }
        }
        Ok(())
    }
    pub async fn start(listen_address: &str) -> Arc<Self> {
        let (chan_sender, mut chan_receiver) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
        let listen_address = listen_address.to_owned();
        let slf = Arc::new(Pipe {
            connections: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            sender: chan_sender,
        });
        let slf_clone = slf.clone();
        tokio::task::spawn(async move {
            let listener = TcpListener::bind(listen_address).await.unwrap();
            loop {
                let (socket, _remote_addr) = listener.accept().await.unwrap();
                let (mut socket_reader, mut socket_writer) = socket.into_split();
                let slfc = slf_clone.clone();
                tokio::spawn(async move {
                    debug!("pipe active");
                    let _ = slfc.handle_data_from_pipe(&mut socket_reader).await;
                    debug!("pipe died");
                    let _ = slfc.sender.send(Vec::new());
                });
                loop {
                    let to_send = match chan_receiver.recv().await {
                        Some(s) => s,
                        None => break,
                    };
                    if to_send.len() == 0 {
                        break;
                    }
                    if socket_writer.write_all(&to_send).await.is_err() {
                        break;
                    }
                }
                debug!("pipe terminated - restarting");
                slf_clone.reset().await;
            }
        });
        slf
    }
    async fn add_connection(&self, c: Connection) {
        let mut lock = self.connections.write().await;
        lock.insert(c.id, c);
    }

    pub async fn connect(
        &self,
        id: u64,
        host: &str,
        port: u16,
        incoming_writer: tokio::sync::mpsc::Sender<Vec<u8>>,
    ) -> Result<()> {
        let con = Connection {
            id,
            incoming_writer,
        };
        self.add_connection(con).await;
        let cmd = pipe_wire::encode_connect(&pipe_wire::Connect {
            host: host.to_owned(),
            port,
            id,
        })?;
        self.sender.send(cmd)?;
        Ok(())
    }
    pub fn send(&self, msg: Vec<u8>) -> Result<()> {
        self.sender.send(msg)?;
        Ok(())
    }
    pub async fn close(&self, id: u64) -> Result<()> {
        let msg = pipe_wire::encode_close(id)?;
        self.send(msg)?;
        let mut clock = self.connections.write().await;
        clock.remove(&id);
        Ok(())
    }
    pub async fn reset(&self) {
        let mut clock = self.connections.write().await;
        clock.clear()
    }
}

/*pub fn test() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        let p = Pipe::start("0.0.0.0:9000").await;
        tokio::time::sleep(Duration::from_secs(3)).await;

        let (sender, mut receiver) = tokio::sync::mpsc::channel(1024);
        tokio::task::spawn(async move {
            loop {
                let rec: Vec<u8> = receiver.recv().await.unwrap();
                //println!("data from pipe {:?}", rec);
                match rec[0] {
                    wire::CMD_CLOSE => {
                        debug!("close");
                    }
                    cmd => {
                        debug!("unknown command {}", cmd);
                    }
                }
            }
        });
        p.connect(1, "127.0.0.1", 1000, sender).await.unwrap();

        tokio::time::sleep(Duration::from_secs(30)).await;
    });
}
*/