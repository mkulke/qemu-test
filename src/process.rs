use crate::config::CONFIG;
use anyhow::{Context, Result, bail};
use log::{debug, error};
use qapi::qmp::{self, RunState};
use qapi::{Qmp, Stream};
use std::io::{BufRead, BufReader};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command};
use std::thread;
use std::time::{Duration, Instant};
use strum::Display;
use strum::EnumString;
use tempfile::TempDir;

const TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_QEMU_BIN: &str = "qemu-system-x86_64";
const DEFAULT_ACCELERATOR: Accelerator = Accelerator::Kvm;

#[derive(Display, EnumString, Clone, Copy)]
#[strum(serialize_all = "lowercase")]
pub(crate) enum Accelerator {
    Kvm,
    Mshv,
}

#[derive(Display)]
#[strum(serialize_all = "lowercase")]
pub(crate) enum Machine {
    Pc,
    Q35,
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

        args.extend(["-accel".into(), cfg.accel.to_string()]);

        args.extend(["-M".into(), cfg.machine.to_string()]);

        if let Some(payload) = &cfg.payload {
            match payload {
                QemuPayload::GuestBin(path) => {
                    args.push("-drive".into());
                    args.push(format!("format=raw,file={},media=disk", path.display()));
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

        debug!("generated QEMU command line: {}", args.join(" "));

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
    accel: Accelerator,
}

pub(crate) struct QemuConfig<'a> {
    temp_dir: &'a TempDir,
    payload: &'a QemuPayload,
    incoming: bool,
    machine: Machine,
    smp: Option<u8>,
}

impl<'a> QemuConfig<'a> {
    pub fn new(temp_dir: &'a TempDir, payload: &'a QemuPayload) -> Self {
        Self {
            temp_dir,
            payload,
            incoming: false,
            machine: Machine::Pc,
            smp: None,
        }
    }

    pub fn new_incoming(temp_dir: &'a TempDir, payload: &'a QemuPayload) -> Self {
        Self {
            temp_dir,
            payload,
            incoming: true,
            machine: Machine::Pc,
            smp: None,
        }
    }

    pub fn with_machine(mut self, machine: Machine) -> Self {
        self.machine = machine;
        self
    }

    pub fn with_smp(mut self, smp: u8) -> Self {
        self.smp = Some(smp);
        self
    }
}

impl QemuProcess {
    pub fn spawn(cfg: QemuConfig) -> Result<Self> {
        let QemuConfig {
            temp_dir,
            payload,
            incoming,
            machine,
            smp,
        } = cfg;
        let qmp_sock_path = temp_dir.path().join("qmp.sock");
        let serial_sock_path = temp_dir.path().join("serial.sock");

        let ram_mb = match payload {
            QemuPayload::GuestBin(_) => 32,
            QemuPayload::Kernel(_) => 256,
        };

        let accel = match CONFIG.accel() {
            Some(value) => value
                .try_into()
                .context(format!("invalid accelerator: {}", value))?,
            None => DEFAULT_ACCELERATOR,
        };

        let cfg = GuestConfig {
            ram_mb,
            serial_sock_path: serial_sock_path.clone(),
            qmp_sock_path: qmp_sock_path.clone(),
            payload: Some(payload.clone()),
            accel,
            machine,
            smp,
            incoming,
        };

        let args: Vec<String> = (&cfg).into();

        let program = CONFIG.qemu_bin().unwrap_or(DEFAULT_QEMU_BIN);

        let child = Command::new(program)
            .args(args)
            .spawn()
            .context(format!("failed to start process: {:?}", program))?;

        debug!("spawned QEMU with PID {}", child.id());

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
            accel,
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
                        debug!("[serial] {line}");
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
        if let Accelerator::Mshv = self.accel {
            debug!(
                "mshv does not support graceful shutdown, killing QEMU (PID {})",
                self.child.id()
            );
            let _ = self.child.kill();
        } else {
            debug!("shutting down QEMU (PID {})", self.child.id());
            if let Err(e) = self.qmp.execute(&qmp::quit {}) {
                error!("QMP quit failed: {e}, killing process");
                let _ = self.child.kill();
            }
        }
        let _ = self.child.wait();
    }
}
