fn read(reader: &mut impl std::io::Read, size: usize) -> std::io::Result<Vec<u8>> {
    let mut out = vec![];
    out.resize(size, 0u8);
    reader.read_exact(&mut out)?;
    return Ok(out);
}

#[derive(Debug)]
pub struct Packet {
    pub id: u32,
    pub opcode: u16,
    pub body: std::vec::Vec<u8>,
}

// header word1 + word2
const BODY_SIZE_ADJ: i64 = 8;

pub fn read_packet(serial: &mut impl std::io::Read) -> Result<Option<Packet>, &'static str> {
    let header_word1 = match read(serial, 4) {
        Ok(x) => x,
        Err(e) => {
            match e.kind() {
                std::io::ErrorKind::UnexpectedEof |
                std::io::ErrorKind::BrokenPipe |
                std::io::ErrorKind::NotConnected |
                std::io::ErrorKind::ConnectionAborted |
                std::io::ErrorKind::ConnectionRefused |
                std::io::ErrorKind::ConnectionReset => {
                    return Ok(None);
                },
                _ => {
                    eprintln!("kind {:?}", e.kind());
                    return Err("header word 1");
                },
            }
        },
    };
    let header_word2 = read(serial, 4).map_err(|_| "header word 2")?;
    let message_size = u16::from_ne_bytes(header_word2[2usize .. 2usize + 2usize].try_into().unwrap());
    let body = read(serial, (message_size as i64 - BODY_SIZE_ADJ) as usize).map_err(|_| "body")?;
    let opcode = u16::from_ne_bytes(header_word2[0usize .. 0usize + 2usize].try_into().unwrap());
    let id = u32::from_ne_bytes(header_word1[0usize .. 0usize + 4usize].try_into().unwrap());
    return Ok(Some(Packet {
        id: id,
        opcode: opcode,
        body: body,
    }));
}

pub fn write_packet(serial: &mut impl std::io::Write, data: &Packet) -> Result<(), &'static str> {
    let mut header_word2 = std::vec::Vec::new();
    header_word2.resize(4usize, 0u8);
    let message_size = (data.body.len() as i64 + BODY_SIZE_ADJ) as u16;
    header_word2[2usize .. 2usize + 2usize].copy_from_slice(&message_size.to_le_bytes());
    header_word2[0usize .. 0usize + 2usize].copy_from_slice(&data.opcode.to_le_bytes());
    let mut header_word1 = std::vec::Vec::new();
    header_word1.resize(4usize, 0u8);
    header_word1[0usize .. 0usize + 4usize].copy_from_slice(&data.id.to_le_bytes());
    serial.write_all(&header_word1).map_err(|_| "header word 1")?;
    serial.write_all(&header_word2).map_err(|_| "header word 2")?;
    serial.write_all(&data.body).map_err(|_| "body")?;
    return Ok(());
}

pub fn read_arg_uint(serial: &mut impl std::io::Read) -> Result<u32, &str> {
    let header = read(serial, 4).map_err(|_| "uint")?;
    return Ok(u32::from_ne_bytes(header[..].try_into().unwrap()));
}

pub fn write_arg_uint(serial: &mut impl std::io::Write, data: u32) -> Result<(), &str> {
    match serial.write_all(&mut data.to_ne_bytes()) {
        Ok(_) => (),
        Err(_) => return Err("string length"),
    };
    return Ok(());
}

pub fn read_arg_string(serial: &mut impl std::io::Read) -> Result<Option<String>, &str> {
    let header = read(serial, 4).map_err(|_| "null terminated string length")?;
    let null_term_len = u32::from_ne_bytes(header[..].try_into().unwrap());
    if null_term_len == 0 {
        return Ok(None);
    }
    let mut body = read(serial, null_term_len.next_multiple_of(4) as usize).map_err(|_| "string body")?;
    body.truncate(null_term_len as usize - 1);
    return Ok(Some(String::from_utf8(body).map_err(|_| "bad utf-8")?));
}

pub fn write_arg_string(serial: &mut impl std::io::Write, data: String) -> Result<(), &str> {
    let mut buf = data.into_bytes();
    buf.push(0);
    let null_term_len = buf.len();
    buf.resize(buf.len().next_multiple_of(4), 0u8);
    serial.write_all(&mut (null_term_len as u32).to_ne_bytes()).map_err(|_| "null terminated string length")?;
    serial.write_all(&mut buf).map_err(|_| "string body")?;
    return Ok(());
}
