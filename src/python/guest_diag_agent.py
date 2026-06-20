#!/usr/bin/env python3

import glob
import os
import socket

PORT = 7777


def read_file(path):
    try:
        with open(path) as f:
            return f.read()
    except Exception as e:
        return f"{type(e).__name__}:{e}"


def procs(name):
    out = []

    for proc_path in glob.glob("/proc/[0-9]*"):
        try:
            with open(f"{proc_path}/comm") as f:
                comm = f.read().strip()

            if comm != name:
                continue

            with open(f"{proc_path}/cmdline", "rb") as f:
                cmdline = f.read().replace(b"\0", b" ")

            cmdline = cmdline.decode(errors="replace").strip()
            out.append(f"{os.path.basename(proc_path)} {cmdline or comm}")
        except Exception:
            pass

    return "\n".join(out)


def stress_ng_pid_exists(pid):
    if not pid.isdigit():
        return "0"

    try:
        with open(f"/proc/{pid.decode()}/comm") as f:
            return "1" if f.read().strip() == "stress-ng" else "0"
    except Exception:
        return "0"


def handle(line):
    key = line.rstrip(b"\r\n")

    if key == b"__diag__:uptime":
        output = read_file("/proc/uptime")
        return (output.replace("\n", "\\n") + "\n").encode()

    if key == b"__diag__:clocksource":
        output = read_file(
            "/sys/devices/system/clocksource/clocksource0/current_clocksource"
        )
        return (output.replace("\n", "\\n") + "\n").encode()

    if key == b"__diag__:stress-ng":
        output = procs("stress-ng")
        return (output.replace("\n", "\\n") + "\n").encode()

    if key.startswith(b"__diag__:stress-ng-pid:"):
        pid = key.removeprefix(b"__diag__:stress-ng-pid:")
        return (stress_ng_pid_exists(pid) + "\n").encode()

    return line


with socket.socket() as listener:
    listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    listener.bind(("0.0.0.0", PORT))
    listener.listen(1)

    conn, _ = listener.accept()
    with conn, conn.makefile("rwb", buffering=0) as stream:
        for line in stream:
            conn.sendall(handle(line))
