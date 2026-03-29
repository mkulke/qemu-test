use anyhow::{Context, Result, bail};
use qapi::qmp::{self, RunState};
use qapi::{Qmp, Stream};
use std::io::{BufRead, BufReader};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command};
use std::thread;
use std::time::{Duration, Instant};
use tempfile::TempDir;

const TIMEOUT: Duration = Duration::from_secs(10);

enum Accelerator {
    Kvm,
}

enum Machine {
    Pc,
}

struct Unconnected {
    path: PathBuf,
}

struct Connected {
    stream: UnixStream,
}

struct Socket<State> {
    state: State,
}

impl Socket<Unconnected> {
    pub fn new(path: PathBuf) -> Self {
        Self {
            state: Unconnected { path },
        }
    }

    pub fn connect(self, timeout: Duration) -> Result<Socket<Connected>> {
        let path = self.state.path;
        let start = Instant::now();
        while !path.exists() {
            if start.elapsed() > timeout {
                bail!("timeout waiting for socket: {}", path.display());
            }
            thread::sleep(Duration::from_millis(50));
        }
        let stream = UnixStream::connect(&path).context("failed to connect to QMP socket")?;

        let socket = Socket {
            state: Connected { stream },
        };

        Ok(socket)
    }
}

struct GuestConfig {
    ram_mb: u16,
    qmp_sock_path: PathBuf,
    serial_sock_path: PathBuf,
    accel: Accelerator,
    machine: Machine,
    payload: Option<QemuPayload>,
    smp: Option<u8>,
    incoming: bool,
}

impl From<&GuestConfig> for Vec<String> {
    fn from(cfg: &GuestConfig) -> Self {
        let mut args = vec![
            "-display".into(),
            "none".into(),
            "-no-reboot".into(),
            "-cpu".into(),
            "host".into(),
        ];

        args.extend([
            "-qmp".into(),
            format!("unix:{},server=on,wait=off", cfg.qmp_sock_path.display()),
        ]);

        args.extend([
            "-serial".into(),
            format!("unix:{},server=on,wait=off", cfg.serial_sock_path.display()),
        ]);

        args.extend([
            "-accel".into(),
            match cfg.accel {
                Accelerator::Kvm => "kvm".into(),
            },
        ]);

        args.extend([
            "-M".into(),
            match cfg.machine {
                Machine::Pc => "pc".into(),
            },
        ]);

        if let Some(payload) = &cfg.payload {
            match payload {
                QemuPayload::GuestBin(path) => {
                    args.push("-drive".into());
                    args.push(format!("format=raw,file={},if=floppy", path.display()));
                }
                QemuPayload::Kernel(path) => {
                    args.extend([
                        "-kernel".into(),
                        format!("{}", path.display()),
                        "-append".into(),
                        "console=ttyS0 earlyprintk=serial panic=-1".into(),
                    ]);
                }
            }
        }

        args.extend(["-m".into(), format!("{}m", cfg.ram_mb)]);

        if let Some(smp) = cfg.smp {
            args.extend(["-smp".into(), smp.to_string()]);
        }

        if cfg.incoming {
            args.extend(["-incoming".into(), "defer".into()]);
        }

        args
    }
}

#[derive(Clone)]
pub(crate) enum QemuPayload {
    GuestBin(PathBuf),
    Kernel(PathBuf),
}

pub(crate) struct QemuProcess {
    child: Child,
    qmp: Qmp<Stream<BufReader<UnixStream>, UnixStream>>,
    serial_reader: BufReader<UnixStream>,
}

pub(crate) struct QemuConfig<'a> {
    temp_dir: &'a TempDir,
    payload: &'a QemuPayload,
    incoming: bool,
}

impl<'a> QemuConfig<'a> {
    pub fn new(temp_dir: &'a TempDir, payload: &'a QemuPayload) -> Self {
        Self {
            temp_dir,
            payload,
            incoming: false,
        }
    }

    pub fn new_incoming(temp_dir: &'a TempDir, payload: &'a QemuPayload) -> Self {
        Self {
            temp_dir,
            payload,
            incoming: true,
        }
    }
}

impl QemuProcess {
    pub fn spawn(cfg: QemuConfig) -> Result<Self> {
        let QemuConfig {
            temp_dir,
            payload,
            incoming,
        } = cfg;
        let qmp_sock_path = temp_dir.path().join("qmp.sock");
        let serial_sock_path = temp_dir.path().join("serial.sock");

        let (ram_mb, smp) = match payload {
            QemuPayload::GuestBin(_) => (32, None),
            QemuPayload::Kernel(_) => (256, Some(2)),
        };

        let cfg = GuestConfig {
            ram_mb,
            serial_sock_path: serial_sock_path.clone(),
            qmp_sock_path: qmp_sock_path.clone(),
            payload: Some(payload.clone()),
            accel: Accelerator::Kvm,
            machine: Machine::Pc,
            smp,
            incoming,
        };

        let args: Vec<String> = (&cfg).into();
        let child = Command::new("qemu-system-x86_64")
            .args(args)
            .spawn()
            .context("failed to start qemu-system-x86_64")?;

        let qmp_sock = Socket::new(qmp_sock_path);
        let stream = qmp_sock.connect(TIMEOUT)?.state.stream;
        let mut qmp = Qmp::new(qapi::Stream::new(
            BufReader::new(stream.try_clone().context("failed to clone stream")?),
            stream,
        ));
        qmp.handshake().context("QMP handshake failed")?;

        let serial_sock = Socket::new(serial_sock_path);
        let stream = serial_sock.connect(TIMEOUT)?.state.stream;
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .context("failed to set serial read timeout")?;
        let serial_reader = BufReader::new(stream);

        let process = Self {
            child,
            qmp,
            serial_reader,
        };
        Ok(process)
    }

    pub fn qmp(&mut self) -> &mut Qmp<Stream<BufReader<UnixStream>, UnixStream>> {
        &mut self.qmp
    }

    pub fn poll_line(&mut self, expected: &str) -> Result<()> {
        let mut output = String::new();
        let start = Instant::now();

        loop {
            if start.elapsed() > TIMEOUT {
                bail!("timeout waiting for {expected}");
            }

            let mut line = String::new();
            match self.serial_reader.read_line(&mut line) {
                Ok(0) => bail!("connection closed while waiting for {expected}"),
                Ok(_) => {
                    output.push_str(&line);
                    if output.contains(expected) {
                        print!("[serial] {line}");
                        return Ok(());
                    }
                }
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut =>
                {
                    continue;
                }
                Err(e) => bail!("serial read error: {e}"),
            }
        }
    }

    pub fn poll_status(&mut self, expected_state: RunState) -> Result<()> {
        let start = std::time::Instant::now();
        loop {
            if start.elapsed() > TIMEOUT {
                bail!("migration timed out");
            }
            let status = self
                .qmp()
                .execute(&qmp::query_status {})
                .context("dest: query_status failed")?;
            if status.status == expected_state {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        Ok(())
    }
}

impl Drop for QemuProcess {
    fn drop(&mut self) {
        if let Err(e) = self.qmp().execute(&qmp::quit {}) {
            eprintln!("failed to quit VM: {e}");
        };

        match self.child.wait() {
            Err(e) => eprintln!("failed to wait for QEMU process: {e}"),
            Ok(exit) => {
                if !exit.success() {
                    eprintln!("QEMU process exited with error: {exit}");
                }
                return;
            }
        }

        if let Err(e) = self.child.kill() {
            eprintln!("failed to kill QEMU process: {e}");
        };
    }
}
