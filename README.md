# QEMU test

Test harness for writing QEMU tests in Rust.

## Build

```bash
make build
```

## Run

Run tests in parallel with 2 jobs (default is sequential):

```bash
make run TEST_JOBS=2
```

## Debug

Run test with debug output:

```bash
make run RUST_LOG=qemu_test::tests::migration=debug
```

Set `KEEP_LOGS=./path/to/logs` to keep logs of all tests in the specified directory for debugging failed runs.

## Configure

The test setup can be configured via environment variables:

- `QEMU_BIN` - path to the QEMU binary to use (default: `qemu-system-x86_64`)
- `TEST_JOBS` - number of parallel test jobs (default: 1)
- `ACCEL` - accelerator to use (default: `kvm`)
- `TEST_FILTER` - filter to select tests to run (default: all tests)
- `KEEP_LOGS` - directory to keep logs of all tests (default: none)

## Filter

This will run only tests with "migration", "simple" or "smp=1" in their label (parameters are expanded into labels):

```bash
make run TEST_JOBS=2 TEST_FILTER=migration,simple,smp=1
```
## Skipping

Tests can be annotated with `#[test_fn(skip = "reason")]` to skip them with a reason. Note that tests that are selected by the filter explicitly will be run even if they are annotated with skip.

## Extend

There is a proc macro to define tests. It can be used to stack test configurations or build a cartesian product of configurations.

This will expand to 1 + 2 = 3 tests:

```rust
#[test_fn(machine = Machine::Pc, smp = 1)]
#[test_fn(machine = Machine::Q35, smp = {2, 4})]
pub(crate) fn test_kernel_boot(machine: Machine, smp: u8) -> Result<()> { ... }
```

## Full OS migration test

Those tests are skipped by default, because they need a bridge and tap devices to be set up on the host and they require superuser privileges to run.

Note that `TEST_JOBS=n` needs at least n*2 tap devices to be available.

### Prepare

Create a bridge and 4 tap devices for a test that can run 2 tests in parallel:

```bash
sudo make setup-bridge
sudo make setup-taps NUM_TAPS=4
```

### Run

With 4 tap devices, 2 tests can run in parallel:

```bash
sudo -E PATH=$PATH make run TEST_JOBS=2 TEST_FILTER=migration_os
```

### Cleanup

The number of tap devices need to be specified so they are removed.

```bash
sudo make teardown-bridge NUM_TAPS=4
```
