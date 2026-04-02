use crate::config::CONFIG;
use anyhow::{Context, Result, bail};
use log::debug;
use qapi::qmp::{self, RunState};
use qapi::{Qmp, Stream};
use regex::Regex;
use std::io::{BufRead, BufReader};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::thread;
use std::time::{Duration, Instant};
use strum::Display;
use strum::EnumString;
use tempfile::TempDir;

const TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_QEMU_BIN: &str = "qemu-system-x86_64";
const DEFAULT_ACCELERATOR: Accelerator = Accelerator::Kvm;

pub(crate) enum ExpectedOutput {
    SubString(String),
    Pattern(Regex),
}

#[derive(Display, EnumString, Clone, Copy)]
#[strum(serialize_all = "lowercase")]
pub(crate) enum Accelerator {
    Kvm,
    Mshv,
}

#[derive(Clone, Display)]
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

#[derive(Clone, Display)]
#[strum(serialize_all = "lowercase")]
pub(crate) enum CpuModel {
    Host,
    Qemu64,
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
    cpu_model: Option<CpuModel>,
    cloud_init: Option<PathBuf>,
    ssh_port: Option<u16>,
    ovmf: Option<PathBuf>,
    io_thread: bool,
}

fn build_osdisk_arg(cfg: &GuestConfig, path: &Path) -> Vec<String> {
    let io_thread_option = if cfg.io_thread { ",iothread=io0" } else { "" };
    vec![
        "-drive".into(),
        format!(
            "file={},format=qcow2,if=none,id=os,snapshot=on",
            path.display()
        ),
        "-device".into(),
        format!("virtio-blk-pci,drive=os{io_thread_option}"),
    ]
}

fn build_kernel_args(kernel: &Path, initrd: Option<&PathBuf>) -> Vec<String> {
    let mut args = vec![
        "-kernel".into(),
        format!("{}", kernel.display()),
        "-append".into(),
        "console=ttyS0 earlyprintk=serial panic=-1".into(),
    ];
    if let Some(initrd) = initrd {
        args.extend(["-initrd".into(), format!("{}", initrd.display())]);
    }
    args
}

impl From<&GuestConfig> for Vec<String> {
    fn from(cfg: &GuestConfig) -> Self {
        let mut args = vec!["-display".into(), "none".into(), "-no-reboot".into()];

        if let Some(cpu) = &cfg.cpu_model {
            args.extend(["-cpu".into(), cpu.to_string()]);
        }

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

        if cfg.io_thread {
            args.extend(["-object".into(), "iothread,id=io0".into()]);
        }

        if let Some(payload) = &cfg.payload {
            match payload {
                QemuPayload::GuestBin(path) => {
                    args.push("-drive".into());
                    args.push(format!("format=raw,file={},media=disk", path.display()));
                }
                QemuPayload::Kernel { kernel, initrd } => {
                    args.extend(build_kernel_args(kernel, initrd.as_ref()))
                }
                QemuPayload::DiskImage(path) => args.extend(build_osdisk_arg(cfg, path)),
            }
        }

        args.extend(["-m".into(), format!("{}m", cfg.ram_mb)]);

        if let Some(smp) = cfg.smp {
            args.extend(["-smp".into(), smp.to_string()]);
        }

        if cfg.incoming {
            args.extend(["-incoming".into(), "defer".into()]);
        }

        if let Some(path) = &cfg.ovmf {
            args.extend([
                "-drive".into(),
                format!("file={},format=raw,if=pflash,readonly=on", path.display()),
            ]);
        }

        if let Some(ci) = &cfg.cloud_init {
            args.extend([
                "-drive".into(),
                format!("file={},format=raw,if=none,id=cidata", ci.display()),
                "-device".into(),
                "virtio-blk-pci,drive=cidata".into(),
            ]);
        }

        if let Some(port) = cfg.ssh_port {
            args.extend([
                "-netdev".into(),
                format!("type=user,id=user-net,hostfwd=tcp::{port}-:22"),
                "-device".into(),
                format!(
                    "virtio-net-pci,mac={},netdev=user-net",
                    crate::cloud_init::GUEST_MAC
                ),
            ]);
        }

        debug!("generated QEMU command line: {}", args.join(" "));

        args
    }
}

#[derive(Clone)]
pub(crate) enum QemuPayload {
    GuestBin(PathBuf),
    Kernel {
        kernel: PathBuf,
        initrd: Option<PathBuf>,
    },
    DiskImage(PathBuf),
}

pub(crate) struct QemuProcess {
    child: Child,
    qmp: Qmp<Stream<BufReader<UnixStream>, UnixStream>>,
    serial_reader: BufReader<UnixStream>,
    accel: Accelerator,
}

#[derive(Clone)]
pub(crate) struct QemuConfig<'a> {
    temp_dir: &'a TempDir,
    payload: &'a QemuPayload,
    incoming: bool,
    machine: Machine,
    smp: Option<u8>,
    cpu_model: Option<CpuModel>,
    cloud_init: Option<PathBuf>,
    ssh_port: Option<u16>,
    ovmf: Option<PathBuf>,
    io_thread: bool,
}

impl<'a> QemuConfig<'a> {
    pub fn new(temp_dir: &'a TempDir, payload: &'a QemuPayload) -> Self {
        Self {
            temp_dir,
            payload,
            incoming: false,
            machine: Machine::Pc,
            smp: None,
            cpu_model: None,
            cloud_init: None,
            ssh_port: None,
            ovmf: None,
            io_thread: false,
        }
    }

    pub fn with_incoming(mut self, temp_dir: &'a TempDir) -> Self {
        self.incoming = true;
        self.temp_dir = temp_dir;
        self
    }

    pub fn with_machine(mut self, machine: Machine) -> Self {
        self.machine = machine;
        self
    }

    pub fn with_smp(mut self, smp: u8) -> Self {
        self.smp = Some(smp);
        self
    }

    pub fn with_cpu_model(mut self, cpu_model: CpuModel) -> Self {
        self.cpu_model = Some(cpu_model);
        self
    }

    pub fn with_cloud_init(mut self, path: PathBuf) -> Self {
        self.cloud_init = Some(path);
        self
    }

    pub fn with_ssh_port(mut self, port: u16) -> Self {
        self.ssh_port = Some(port);
        self
    }

    pub fn with_ovmf(mut self, path: PathBuf) -> Self {
        self.ovmf = Some(path);
        self
    }

    pub fn with_io_thread(mut self) -> Self {
        self.io_thread = true;
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
            cpu_model,
            cloud_init,
            ssh_port,
            ovmf,
            io_thread,
        } = cfg;
        let qmp_sock_path = temp_dir.path().join("qmp.sock");
        let serial_sock_path = temp_dir.path().join("serial.sock");

        let ram_mb = match payload {
            QemuPayload::GuestBin(_) => 32,
            QemuPayload::Kernel { .. } => 256,
            QemuPayload::DiskImage(_) => 1024,
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
            cpu_model,
            cloud_init,
            ssh_port,
            ovmf,
            io_thread,
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

    pub fn poll_line(&mut self, expected: ExpectedOutput) -> Result<()> {
        self.poll_line_timeout(expected, TIMEOUT)
    }

    pub fn poll_line_timeout(&mut self, expected: ExpectedOutput, timeout: Duration) -> Result<()> {
        let start = Instant::now();

        loop {
            if start.elapsed() > timeout {
                bail!("timeout waiting for expected output");
            }

            let mut line = String::new();
            match self.serial_reader.read_line(&mut line) {
                Ok(0) => bail!("connection closed while waiting for expected output"),
                Ok(_) => {
                    debug!("[serial] {}", line.trim_end());
                    match expected {
                        ExpectedOutput::SubString(ref s) => {
                            if line.contains(s) {
                                return Ok(());
                            }
                        }
                        ExpectedOutput::Pattern(ref r) => {
                            if r.is_match(&line) {
                                return Ok(());
                            }
                        }
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
            std::thread::sleep(Duration::from_millis(200));
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
            // qmp::quit may fail to deserialize the response (untagged enum on q35 + ovmf + qemu 6.2),
            if let Err(e) = self.qmp.execute(&qmp::quit {}) {
                debug!("qmp::quit failed: {e}");
                debug!("falling back to killing QEMU (PID {})", self.child.id());
                thread::sleep(Duration::from_millis(200));
                let _ = self.child.kill();
            }
        }
        let _ = self.child.wait();
    }
}
