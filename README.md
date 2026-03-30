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

Run test with debug output:

```bash
make run RUST_LOG=qemu_test::tests::migration=debug
```

## Configure

The test setup can be configured via environment variables:

- `QEMU_BIN` - path to the QEMU binary to use (default: `qemu-system-x86_64`)
- `TEST_JOBS` - number of parallel test jobs (default: 1)
- `ACCEL` - accelerator to use (default: `kvm`)

## Extend

There is a proc macro to define tests. It can be used to stack test configurations or build a cartesian product of configurations.

This will expand to 1 + 2 = 3 tests:

```rust
#[test_fn(machine = Machine::Pc, smp = 1)]
#[test_fn(machine = Machine::Q35, smp = {2, 4})]
pub(crate) fn test_kernel_boot(machine: Machine, smp: u8) -> Result<()> { ... }
```
