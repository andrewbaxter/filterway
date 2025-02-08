#![feature(unix_socket_ancillary_data)]

use {
    aargvark::{
        vark,
        Aargvark,
    },
    proto::read_arg_string,
    rustix::{
        fd::{
            AsFd,
            FromRawFd,
            OwnedFd,
            RawFd,
        },
        fs::{
            flock,
            OpenOptionsExt,
        },
    },
    std::{
        collections::HashMap,
        fmt::Display,
        fs::{
            remove_file,
            File,
        },
        io::{
            Cursor,
            IoSlice,
            IoSliceMut,
        },
        os::unix::net::{
            AncillaryData,
            SocketAncillary,
            UnixListener,
            UnixStream,
        },
        path::PathBuf,
        process::exit,
        sync::{
            Arc,
            Mutex,
        },
        thread::spawn,
    },
};

pub mod proto;

#[derive(Aargvark, Clone)]
struct Args {
    /// Full path to primary compositor Wayland socket (like `/run/user/1000/wayland-0`)
    #[vark(flag = "upstream")]
    upstream: PathBuf,
    /// Full path for new Wayland socket
    #[vark(flag = "downstream")]
    downstream: PathBuf,
    /// Force all xdg toplevels to have the same app id
    #[vark(flag = "app-id")]
    app_id: String,
    /// Prefix the app id instead of replacing
    prefix: Option<()>,
    /// Print debug messages
    debug: Option<()>,
}

trait Errorize<T> {
    fn context(self, text: &str) -> Result<T, String>;
}

impl<T, E: Display> Errorize<T> for Result<T, E> {
    fn context(self, text: &str) -> Result<T, String> {
        match self {
            Ok(x) => Ok(x),
            Err(e) => Err(format!("{}: {}", text, e)),
        }
    }
}

struct AncillaryReader<'a> {
    reader: &'a UnixStream,
    ancillary_mem: &'a mut [u8],
    fds: &'a mut Vec<RawFd>,
}

impl<'a> std::io::Read for AncillaryReader<'a> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let mut ancillary = SocketAncillary::new(&mut self.ancillary_mem);
        let res = self.reader.recv_vectored_with_ancillary(&mut [IoSliceMut::new(buf)], &mut ancillary);
        if ancillary.truncated() {
            panic!("Ancillary buffer too small");
        }
        for m in ancillary.messages() {
            let Ok(AncillaryData::ScmRights(m)) = m else {
                continue;
            };
            self.fds.extend(m);
        }
        return res;
    }
}

struct AncillaryWriter<'a, 'b> {
    writer: &'a UnixStream,
    ancillary: SocketAncillary<'b>,
}

impl<'a, 'b> AncillaryWriter<'a, 'b> {
    fn new(writer: &'a UnixStream, ancillary_mem: &'b mut [u8], fds: &Vec<RawFd>) -> Self {
        let mut ancillary = SocketAncillary::new(ancillary_mem);
        ancillary.add_fds(fds.as_ref());
        return Self {
            writer: writer,
            ancillary: ancillary,
        };
    }
}

impl<'a, 'b> std::io::Write for AncillaryWriter<'a, 'b> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let res = self.writer.send_vectored_with_ancillary(&mut [IoSlice::new(buf)], &mut self.ancillary);
        self.ancillary.clear();
        return res;
    }

    fn flush(&mut self) -> std::io::Result<()> {
        return self.writer.flush();
    }
}

fn main() {
    fn inner() -> Result<(), String> {
        let args = vark::<Args>();
        let lock_path = args.downstream.with_extension("lock");
        let filelock =
            File::options()
                .mode(0o660)
                .write(true)
                .create(true)
                .custom_flags(libc::O_CLOEXEC)
                .open(&lock_path)
                .context("Error opening lock file")?;
        flock(
            filelock.as_fd(),
            rustix::fs::FlockOperation::NonBlockingLockExclusive,
        ).context("Error getting exclusive lock for downstream listener, is another compositor already listening?")?;
        let _defer = defer::defer(|| {
            _ = remove_file(&lock_path);
        });
        _ = remove_file(&args.downstream);
        let downstream = UnixListener::bind(&args.downstream).context("Error creating downstream listener")?;
        let _defer1 = defer::defer(|| {
            _ = remove_file(&args.downstream);
        });

        // Listen for connections
        loop {
            let (downstream, _) = downstream.accept().context("Error accepting downstream connection")?;
            let upstream = UnixStream::connect(&args.upstream).context("Error creating upstream connection")?;

            #[derive(Clone, Copy, Debug)]
            enum ObjType {
                Display,
                Registry,
                XdgWmBase {
                    ver: u32,
                },
                XdgSurface {
                    ver: u32,
                },
                XdgToplevel {
                    ver: u32,
                },
            }

            let objects = Arc::new(Mutex::new(HashMap::new()));
            objects.lock().unwrap().insert(1, ObjType::Display);
            let xdgwmbase_type_id = Arc::new(Mutex::new(None));
            spawn({
                let downstream = downstream.try_clone().unwrap();
                let mut upstream = upstream.try_clone().unwrap();
                let objects = objects.clone();
                let xdgwmbase_type_id = xdgwmbase_type_id.clone();
                let args = args.clone();
                move || {
                    let _defer = defer::defer({
                        let downstream = downstream.try_clone().unwrap();
                        let upstream = upstream.try_clone().unwrap();
                        move || {
                            _ = downstream.shutdown(std::net::Shutdown::Both);
                            _ = upstream.shutdown(std::net::Shutdown::Both);
                        }
                    });
                    match (|| -> Result<(), String> {
                        let mut send_extra = vec![];
                        let mut ancillary_mem = [0u8; 128];
                        let mut ancillary_accum = vec![];
                        loop {
                            // Wait for next message
                            let Some(mut packet) = proto::read_packet(&mut AncillaryReader {
                                reader: &downstream,
                                ancillary_mem: &mut ancillary_mem,
                                fds: &mut ancillary_accum,
                            }).context("Error reading message")? else {
                                break;
                            };

                            // Track and prepare manipulations
                            {
                                let mut objects = objects.lock().unwrap();
                                let o = objects.get(&packet.id).cloned();
                                if args.debug.is_some() {
                                    eprintln!(
                                        "Received packet from downstream for tracked object {:?} with {} ancillary FDs: {:?}",
                                        o,
                                        ancillary_accum.len(),
                                        packet
                                    );
                                }
                                if let Some(o) = o {
                                    match o {
                                        ObjType::Display => {
                                            match packet.opcode {
                                                // Get registry
                                                1 => {
                                                    let mut cursor = Cursor::new(&packet.body);
                                                    let obj_id =
                                                        proto::read_arg_uint(
                                                            &mut cursor,
                                                        ).context("Error reading registry id")?;
                                                    objects.insert(obj_id, ObjType::Registry);
                                                },
                                                _ => { },
                                            }
                                        },
                                        ObjType::Registry => {
                                            match packet.opcode {
                                                // Bind
                                                0 => {
                                                    let mut cursor = Cursor::new(&packet.body);
                                                    let obj_type_id =
                                                        proto::read_arg_uint(
                                                            &mut cursor,
                                                        ).context("Error/eof reading bind object type id")?;

                                                    // Arbitrary snowflake magic param - interface name
                                                    proto::read_arg_string(&mut cursor).context("Error reading bind message type string")?;

                                                    // Arbitrary snowflake magic param - version
                                                    let version = proto::read_arg_uint(&mut cursor).context("Error reading bind message version")?;
                                                    let obj_id =
                                                        proto::read_arg_uint(
                                                            &mut cursor,
                                                        ).context("Error/eof reading bind object id")?;
                                                    if let Some((want_type_id, _version)) =
                                                        *xdgwmbase_type_id.lock().unwrap() {
                                                        if obj_type_id == want_type_id {
                                                            objects.insert(obj_id, ObjType::XdgWmBase {
                                                                // prefer the magic param version because it's nearer to the use location...
                                                                ver: version,
                                                            });
                                                        }
                                                    }
                                                },
                                                _ => { },
                                            }
                                        },
                                        ObjType::XdgWmBase { ver } => {
                                            match ver {
                                                0 ..= 6 => match packet.opcode {
                                                    // Get surface
                                                    2 => {
                                                        let mut cursor = Cursor::new(&packet.body);
                                                        let obj_id =
                                                            proto::read_arg_uint(
                                                                &mut cursor,
                                                            ).context("Error reading xdg wm base create surface id")?;
                                                        objects.insert(obj_id, ObjType::XdgSurface { ver: ver });
                                                    },
                                                    _ => (),
                                                },
                                                _ => panic!(
                                                    "Unsupported xdg_wm_base object version {}_{}_{}_{}",
                                                    ver,
                                                    ver,
                                                    ver,
                                                    ver
                                                ),
                                            }
                                        },
                                        ObjType::XdgSurface { ver } => {
                                            match ver {
                                                0 ..= 6 => match packet.opcode {
                                                    // Create toplevel
                                                    1 => {
                                                        let mut cursor = Cursor::new(&packet.body);
                                                        let obj_id =
                                                            proto::read_arg_uint(
                                                                &mut cursor,
                                                            ).context(
                                                                "Error reading xdg surface create toplevel id",
                                                            )?;
                                                        objects.insert(obj_id, ObjType::XdgToplevel { ver: ver });
                                                    },
                                                    _ => (),
                                                },
                                                _ => panic!("Unsupported xdg_surface object version {}", ver),
                                            }
                                        },
                                        ObjType::XdgToplevel { ver } => {
                                            match ver {
                                                0 ..= 6 => match packet.opcode {
                                                    // set_app_id => replace
                                                    3 => {
                                                        let read_app_id =
                                                            read_arg_string(
                                                                &mut packet.body.as_slice(),
                                                            ).context("Error reading app id message body")?;
                                                        packet.body.clear();
                                                        proto::write_arg_string(
                                                            &mut packet.body,
                                                            if args.prefix.is_some() {
                                                                format!(
                                                                    "{}{}",
                                                                    args.app_id,
                                                                    read_app_id.unwrap_or_default()
                                                                )
                                                            } else {
                                                                args.app_id.clone()
                                                            },
                                                        ).unwrap();
                                                    },
                                                    _ => (),
                                                },
                                                _ => panic!("Unsupported xdg_toplevel object version {}", ver),
                                            }
                                        },
                                    }
                                }
                            }

                            // Forward message with retractions/additions
                            proto::write_packet(
                                &mut AncillaryWriter::new(&mut upstream, &mut ancillary_mem, &ancillary_accum),
                                &packet,
                            ).context("Error writing message")?;
                            for fd in ancillary_accum.drain(..) {
                                drop(unsafe {
                                    OwnedFd::from_raw_fd(fd)
                                });
                            }
                            for m in send_extra.drain(..) {
                                if args.debug.is_some() {
                                    eprintln!("Sending synthetic request upstream: {:?}", m);
                                }
                                proto::write_packet(&mut upstream, &m).context("Error writing message")?;
                            }
                        }
                        return Ok(());
                    })() {
                        Ok(_) => { },
                        Err(e) => {
                            eprintln!("Warning, client->server thread exiting with error: {}", e);
                        },
                    }
                }
            });
            spawn({
                let mut downstream = downstream.try_clone().unwrap();
                let mut upstream = upstream.try_clone().unwrap();
                let objects = objects.clone();
                move || {
                    let _defer = defer::defer({
                        let downstream = downstream.try_clone().unwrap();
                        let upstream = upstream.try_clone().unwrap();
                        move || {
                            _ = downstream.shutdown(std::net::Shutdown::Both);
                            _ = upstream.shutdown(std::net::Shutdown::Both);
                        }
                    });
                    match (|| -> Result<(), String> {
                        let mut ancillary_mem = [0u8; 128];
                        let mut ancillary_accum = vec![];
                        let mut cache_reg_id = None;
                        loop {
                            // Read next packet
                            let Some(packet) = proto::read_packet(&mut AncillaryReader {
                                reader: &mut upstream,
                                ancillary_mem: &mut ancillary_mem,
                                fds: &mut ancillary_accum,
                            }).context("Error reading message")? else {
                                break;
                            };
                            if args.debug.is_some() {
                                eprintln!(
                                    "Received packet from upstream with {} ancillary FDs: {:?}",
                                    ancillary_accum.len(),
                                    packet
                                );
                            }

                            // Tracking and manipulation
                            match (packet.id, packet.opcode) {
                                // Ack delete, hardcoded display
                                (1, 1) => {
                                    let mut cursor = Cursor::new(&packet.body);
                                    let obj_id =
                                        proto::read_arg_uint(
                                            &mut cursor,
                                        ).context("Error reading display delete obj id")?;
                                    objects.lock().unwrap().remove(&obj_id);
                                },
                                _ => { },
                            }
                            if let Some(reg_id) = match &cache_reg_id {
                                Some(r) => Some(*r),
                                None => {
                                    if let Some(ObjType::Registry) = objects.lock().unwrap().get(&packet.id) {
                                        cache_reg_id = Some(packet.id);
                                        Some(packet.id)
                                    } else {
                                        None
                                    }
                                },
                            } {
                                if reg_id == packet.id {
                                    // global
                                    if packet.opcode == 0 {
                                        let mut cursor = Cursor::new(&packet.body);
                                        let type_id =
                                            proto::read_arg_uint(
                                                &mut cursor,
                                            ).context("Error reading global message type id")?;
                                        let type_str =
                                            proto::read_arg_string(
                                                &mut cursor,
                                            ).context("Error reading global message type string")?;
                                        let version =
                                            proto::read_arg_uint(
                                                &mut cursor,
                                            ).context("Error reading global message version")?;
                                        if type_str.as_ref().map(|x| x.as_str()) == Some("xdg_wm_base") {
                                            *xdgwmbase_type_id.lock().unwrap() = Some((type_id, version));
                                        }
                                    }
                                }
                            }

                            // Forward messages
                            proto::write_packet(
                                &mut AncillaryWriter::new(&mut downstream, &mut ancillary_mem, &ancillary_accum),
                                &packet,
                            ).context("Error writing message")?;
                            for fd in ancillary_accum.drain(..) {
                                drop(unsafe {
                                    OwnedFd::from_raw_fd(fd)
                                });
                            }
                        }
                        return Ok(());
                    })() {
                        Ok(_) => { },
                        Err(e) => {
                            eprintln!("Warning, server->client thread exiting with error: {}", e);
                        },
                    }
                }
            });
        }
    }

    match inner() {
        Ok(_) => { },
        Err(e) => {
            eprintln!("{}", e);
            exit(1);
        },
    }
}
