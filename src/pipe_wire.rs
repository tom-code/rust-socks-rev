use anyhow::Result;
use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use core::str;
use std::io::{Read, Write};

pub const CMD_CONNECT: u8 = 1;
pub const CMD_CLOSE: u8 = 2;
pub const CMD_DATA: u8 = 3;
pub const CMD_CONNECT_ACK: u8 = 4;


pub const CONNECT_STATUS_OK: u8 = 0;
pub const CONNECT_STATUS_ERR: u8 = 1;

#[derive(Debug)]
pub struct Connect {
    pub host: String,
    pub port: u16,
    pub id: u64,
}

#[derive(Debug)]
pub struct Header {
    pub size: u32,
    pub id: u64,
}

pub fn encode_connect(c: &Connect) -> Result<Vec<u8>> {
    let mut buf = Vec::with_capacity(1024);
    let host = c.host.as_bytes();
    buf.write_u32::<LittleEndian>(1 + host.len() as u32 + 2 + 1)?;
    buf.write_u64::<LittleEndian>(c.id)?;
    buf.write_u8(CMD_CONNECT)?;
    buf.write_u8(host.len() as u8)?;
    buf.write(host)?;
    buf.write_u16::<LittleEndian>(c.port)?;

    Ok(buf)
}

pub fn encode_connect_ack(id: u64, status: u8) -> Result<Vec<u8>> {
    let mut buf = Vec::with_capacity(1024);
    buf.write_u32::<LittleEndian>(2)?;
    buf.write_u64::<LittleEndian>(id)?;
    buf.write_u8(CMD_CONNECT_ACK)?;
    buf.write_u8(status)?;
    Ok(buf)
}

pub fn decode_connect(data: &[u8]) -> Result<Connect> {
    let mut cursor = std::io::Cursor::new(data);
    let hostlen = cursor.read_u8()?;
    let mut host = vec![0; hostlen as usize];
    cursor.read_exact(&mut host)?;
    let port = cursor.read_u16::<LittleEndian>()?;
    Ok(Connect {
        host: str::from_utf8(&host)?.to_owned(),
        port,
        id: 0,
    })
}

pub fn encode_close(id: u64) -> Result<Vec<u8>> {
    let mut buf = Vec::with_capacity(1024);
    buf.write_u32::<LittleEndian>(1)?;
    buf.write_u64::<LittleEndian>(id)?;
    buf.write_u8(CMD_CLOSE)?;
    Ok(buf)
}

pub fn encode_data(id: u64, data: &[u8]) -> Result<Vec<u8>> {
    let mut buf = Vec::with_capacity(1024);
    buf.write_u32::<LittleEndian>(data.len() as u32 + 1)?;
    buf.write_u64::<LittleEndian>(id)?;
    buf.write_u8(CMD_DATA)?;
    buf.write(data)?;
    Ok(buf)
}

pub fn decode_data(data: &[u8]) -> Result<Vec<u8>> {
    //let mut cursor = std::io::Cursor::new(data);
    let val = data;
    Ok(val.to_owned())
}

pub fn decode_header(data: &[u8]) -> Result<Header> {
    let mut cursor = std::io::Cursor::new(data);
    let size = cursor.read_u32::<LittleEndian>()?;
    let id = cursor.read_u64::<LittleEndian>()?;
    Ok(Header { size, id })
}

#[test]
fn a_test() {
    let c = encode_connect(&Connect {
        host: "a.b.c".to_owned(),
        port: 123,
        id: 999,
    })
    .unwrap();
    let d = decode_connect(&c[5..]).unwrap();
    println!("{:?}", d);
}
