use crate::config::CONFIG;
use crate::util::NetConfig;
use anyhow::{Context, Result, bail};
use log::debug;
use qapi::qmp::{self, RunState};
use qapi::{Qmp, Stream};
use regex::Regex;
use std::fs::File;
use std::io::{BufRead, BufReader, ErrorKind};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::thread;
use std::thread::sleep;
use std::time::{Duration, Instant};
use strum::Display;
use strum::EnumString;
use tempfile::TempDir;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_QEMU_BIN: &str = "qemu-system-x86_64";

pub(crate) enum ExpectedOutput {
    SubString(String),
    Pattern(Regex),
}

impl ExpectedOutput {
    pub fn matches(&self, line: &str) -> bool {
        match self {
            ExpectedOutput::SubString(s) => line.contains(s),
            ExpectedOutput::Pattern(r) => r.is_match(line),
        }
    }
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

#[derive(Clone, Display)]
#[strum(serialize_all = "lowercase")]
pub(crate) enum RtcClock {
    Vm,
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
    serial_log_path: PathBuf,
    accel: Accelerator,
    machine: Machine,
    payload: Option<QemuPayload>,
    smp: Option<u8>,
    incoming: bool,
    cpu_model: Option<CpuModel>,
    cloud_init: Option<PathBuf>,
    net: Option<NetConfig>,
    ovmf: Option<PathBuf>,
    io_thread: bool,
    rtc_clock: Option<RtcClock>,
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
            format!("file:{}", cfg.serial_log_path.display()),
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

        if let Some(rtc_clock) = &cfg.rtc_clock {
            args.extend(["-rtc".into(), format!("clock={}", rtc_clock)]);
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

        if let Some(net_config) = &cfg.net {
            let mac = net_config.mac();
            match net_config {
                NetConfig::UserNet { .. } => {
                    args.extend([
                        "-netdev".into(),
                        "type=user,id=net0,hostfwd=tcp::0-:22".into(),
                    ]);
                }
                NetConfig::Tap { ifname, .. } => {
                    args.extend([
                        "-netdev".into(),
                        format!("tap,id=net0,ifname={ifname},script=no,downscript=no"),
                    ]);
                }
            }
            args.extend([
                "-device".into(),
                format!("virtio-net-pci,mac={mac},netdev=net0"),
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
    serial_reader: BufReader<File>,
    serial_log_path: PathBuf,
    accel: Accelerator,
}

#[derive(Clone)]
pub(crate) struct QemuConfig<'a> {
    temp_dir: &'a TempDir,
    payload: &'a QemuPayload,
    ram_mb: u16,
    incoming: bool,
    machine: Machine,
    smp: Option<u8>,
    cpu_model: Option<CpuModel>,
    cloud_init: Option<PathBuf>,
    net: Option<NetConfig>,
    ovmf: Option<PathBuf>,
    io_thread: bool,
    rtc_clock: Option<RtcClock>,
}

impl<'a> QemuConfig<'a> {
    pub fn new(temp_dir: &'a TempDir, payload: &'a QemuPayload) -> Self {
        let ram_mb = match payload {
            QemuPayload::GuestBin(_) => 32,
            QemuPayload::Kernel { .. } => 256,
            QemuPayload::DiskImage(_) => 1024,
        };
        Self {
            temp_dir,
            payload,
            ram_mb,
            incoming: false,
            machine: Machine::Pc,
            smp: None,
            cpu_model: None,
            cloud_init: None,
            net: None,
            ovmf: None,
            io_thread: false,
            rtc_clock: None,
        }
    }

    pub fn ram_mb(&self) -> u16 {
        self.ram_mb
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

    pub fn with_net(mut self, config: NetConfig) -> Self {
        self.net = Some(config);
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

    pub fn with_rtc_clock(mut self, clock: RtcClock) -> Self {
        self.rtc_clock = Some(clock);
        self
    }
}

enum ChunkResult {
    Line(String),
    Progress,
    NoProgress,
}

impl QemuProcess {
    pub fn spawn(cfg: QemuConfig) -> Result<Self> {
        let QemuConfig {
            temp_dir,
            payload,
            ram_mb,
            incoming,
            machine,
            smp,
            cpu_model,
            cloud_init,
            net,
            ovmf,
            io_thread,
            rtc_clock,
        } = cfg;
        let qmp_sock_path = temp_dir.path().join("qmp.sock");
        let serial_log_path = temp_dir.path().join("serial.log");

        let accel = CONFIG.accel()?;

        let cfg = GuestConfig {
            ram_mb,
            qmp_sock_path: qmp_sock_path.clone(),
            payload: Some(payload.clone()),
            accel,
            machine,
            smp,
            incoming,
            cpu_model,
            cloud_init,
            net,
            ovmf,
            io_thread,
            rtc_clock,
            serial_log_path,
        };

        let args: Vec<String> = (&cfg).into();

        let program = CONFIG.qemu_bin().unwrap_or(DEFAULT_QEMU_BIN);

        let child = Command::new(program)
            .args(args)
            .spawn()
            .context(format!("failed to start process: {:?}", program))?;

        debug!("spawned QEMU with PID {}", child.id());

        let qmp_sock = Socket::new(qmp_sock_path);
        let stream = qmp_sock.connect(DEFAULT_TIMEOUT)?.state.stream;
        let mut qmp = Qmp::new(qapi::Stream::new(
            BufReader::new(stream.try_clone().context("failed to clone stream")?),
            stream,
        ));
        qmp.handshake().context("QMP handshake failed")?;

        let serial_log =
            File::open(&cfg.serial_log_path).context("failed to open serial log file")?;
        let serial_reader = BufReader::new(serial_log);
        let process = Self {
            child,
            qmp,
            serial_reader,
            serial_log_path: cfg.serial_log_path.clone(),
            accel,
        };
        Ok(process)
    }

    /// Copy the serial log to `KEEP_LOGS/<label>-<pid>.log` for post-mortem analysis.
    pub fn save_serial_log(&self) {
        let Some(dir) = CONFIG.keep_logs() else {
            return;
        };
        let label = crate::CURRENT_TEST_LABEL.with(|l| l.borrow().clone());
        if label.is_empty() {
            return;
        }
        let dest = PathBuf::from(dir);
        if let Err(e) = std::fs::create_dir_all(&dest) {
            debug!("failed to create KEEP_LOGS dir: {e}");
            return;
        }
        let sanitized = label.replace(['(', ')', ',', ' ', '='], "_");
        let pid = self.child.id();
        let dest_file = dest.join(format!("{sanitized}-{pid}.log"));
        match std::fs::copy(&self.serial_log_path, &dest_file) {
            Ok(_) => debug!("saved serial log to {}", dest_file.display()),
            Err(e) => debug!("failed to save serial log: {e}"),
        }
    }

    pub fn qmp(&mut self) -> &mut Qmp<Stream<BufReader<UnixStream>, UnixStream>> {
        &mut self.qmp
    }

    /// Query the OS-assigned SSH port from QEMU's user-net hostfwd.
    pub fn ssh_port(&mut self) -> Result<u16> {
        let output: String = self
            .qmp
            .execute(&qmp::human_monitor_command {
                command_line: "info usernet".into(),
                cpu_index: None,
            })
            .context("failed to query usernet info")?;
        debug!("info usernet: {output}");
        for line in output.lines() {
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.len() >= 6 && fields[0].contains("HOST_FORWARD") && fields[5] == "22" {
                return fields[3]
                    .parse()
                    .context("failed to parse SSH port from usernet info");
            }
        }
        bail!("no SSH host forward found in usernet info")
    }

    pub fn poll_line(&mut self, expected: ExpectedOutput) -> Result<()> {
        self.poll_line_timeout(expected, DEFAULT_TIMEOUT)
    }

    fn read_chunk(&mut self, partial: &mut Vec<u8>) -> Result<ChunkResult> {
        let mut buf = Vec::new();

        match self.serial_reader.read_until(b'\n', &mut buf) {
            Ok(0) => Ok(ChunkResult::NoProgress),
            Ok(_) => {
                partial.extend_from_slice(&buf);

                if !partial.ends_with(b"\n") {
                    return Ok(ChunkResult::Progress);
                }

                let line = String::from_utf8_lossy(partial).into_owned();
                partial.clear();

                Ok(ChunkResult::Line(line))
            }

            Err(e) if e.kind() == ErrorKind::WouldBlock || e.kind() == ErrorKind::TimedOut => {
                Ok(ChunkResult::NoProgress)
            }

            Err(e) => bail!("serial file read error: {e}"),
        }
    }

    pub fn poll_line_timeout(&mut self, expected: ExpectedOutput, timeout: Duration) -> Result<()> {
        let start = Instant::now();
        let mut partial = Vec::new();

        loop {
            if crate::SHUTDOWN.load(std::sync::atomic::Ordering::Relaxed) {
                bail!("interrupted");
            }
            if start.elapsed() > timeout {
                bail!("timeout waiting for expected output");
            }

            match self.read_chunk(&mut partial)? {
                ChunkResult::Line(line) => {
                    debug!("[serial] {}", line.trim_end());
                    if expected.matches(&line) {
                        return Ok(());
                    }
                }
                ChunkResult::Progress => {
                    // keep reading until we get a full line
                    continue;
                }
                ChunkResult::NoProgress => {
                    // nothing read, wait for more data
                    sleep(Duration::from_millis(100));
                }
            }
        }
    }

    pub fn poll_status(&mut self, expected_state: RunState, timeout: Duration) -> Result<()> {
        let start = std::time::Instant::now();
        loop {
            if crate::SHUTDOWN.load(std::sync::atomic::Ordering::Relaxed) {
                bail!("interrupted");
            }
            if start.elapsed() > timeout {
                bail!("timed out waiting for expected status");
            }
            let status = self
                .qmp()
                .execute(&qmp::query_status {})
                .context("query_status failed")?;
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
        self.save_serial_log();
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
