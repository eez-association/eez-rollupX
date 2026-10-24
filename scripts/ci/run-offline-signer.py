#!/usr/bin/env python3
"""Execute a local test signer with kernel-enforced outbound networking denied.

The process can create TCP listeners and answer accepted connections. New
connections, datagram/raw/Unix sockets and io_uring are denied with EPERM. This
lets the existing native gRPC tests prove offline operation without mocking an
RPC provider. The filter survives exec and all worker threads; installation
failure is fatal. This is a test launcher, not a general process sandbox.
"""

import ctypes
import errno
import os
import platform
import stat
import sys


class Filter(ctypes.Structure):
    _fields_ = [("code", ctypes.c_ushort), ("jt", ctypes.c_ubyte),
                ("jf", ctypes.c_ubyte), ("k", ctypes.c_uint)]


class Program(ctypes.Structure):
    _fields_ = [("length", ctypes.c_ushort), ("filter", ctypes.POINTER(Filter))]


def deny_outbound():
    # Linux seccomp_data: syscall nr at 0, architecture at 4, six u64 args at 16.
    architectures = {
        "x86_64": (0xC000003E, 41, 42, 44, 46, 307, 425),
        "aarch64": (0xC00000B7, 198, 203, 206, 211, 269, 425),
    }
    if sys.platform != "linux" or platform.machine() not in architectures:
        raise RuntimeError("offline signer test requires Linux x86_64 or aarch64")
    arch, socket_nr, connect_nr, sendto_nr, sendmsg_nr, sendmmsg_nr, uring_nr = architectures[platform.machine()]
    allow, deny, kill = 0x7FFF0000, 0x00050000 | errno.EPERM, 0x80000000
    instructions = [
        (0x20, 0, 0, 4),                 # Load architecture.
        (0x15, 1, 0, arch),
        (0x06, 0, 0, kill),              # Reject a different syscall ABI.
        (0x20, 0, 0, 0),
        (0x45, 0, 1, 0x40000000),       # Never admit the x32 ABI.
        (0x06, 0, 0, kill),
        (0x15, 0, 8, socket_nr),
        (0x20, 0, 0, 16),                # socket domain: IPv4 or IPv6 only.
        (0x15, 1, 0, 2),
        (0x15, 0, 4, 10),
        (0x20, 0, 0, 24),                # socket type, ignoring CLOEXEC/NONBLOCK.
        (0x54, 0, 0, 0xF),
        (0x15, 0, 1, 1),                 # SOCK_STREAM only.
        (0x06, 0, 0, allow),
        (0x06, 0, 0, deny),
        (0x20, 0, 0, 0),
        (0x15, 0, 1, connect_nr),
        (0x06, 0, 0, deny),
        (0x15, 0, 1, uring_nr),          # No alternate async connect interface.
        (0x06, 0, 0, deny),
    ]
    # TCP Fast Open can initiate a connection through sendto/sendmsg instead
    # of connect. Ordinary sends on accepted sockets must still work.
    for syscall, flags_offset in [(sendto_nr, 40), (sendmsg_nr, 32), (sendmmsg_nr, 40)]:
        instructions.extend([
            (0x15, 0, 4, syscall),
            (0x20, 0, 0, flags_offset),
            (0x45, 0, 1, 0x20000000),    # MSG_FASTOPEN
            (0x06, 0, 0, deny),
            (0x20, 0, 0, 0),
        ])
    instructions.append((0x06, 0, 0, allow))
    filters = (Filter * len(instructions))(*(Filter(*entry) for entry in instructions))
    program = Program(len(filters), filters)
    libc = ctypes.CDLL(None, use_errno=True)
    # no_new_privs allows an unprivileged process to install a persistent filter.
    if libc.prctl(38, 1, 0, 0, 0) != 0 or libc.prctl(22, 2, ctypes.byref(program), 0, 0) != 0:
        raise OSError(ctypes.get_errno(), "cannot install offline signer network filter")


def main():
    if len(sys.argv) < 2 or not os.path.isabs(sys.argv[1]):
        raise RuntimeError("usage: run-offline-signer.py /absolute/executable [arguments]")
    # Do not inherit an already connected descriptor through the test launcher.
    for fd in (0, 1, 2):
        if stat.S_ISSOCK(os.fstat(fd).st_mode):
            raise RuntimeError("offline signer standard streams must not be sockets")
    with os.scandir("/proc/self/fd") as entries:
        descriptors = [int(entry.name) for entry in entries if int(entry.name) > 2]
    for fd in descriptors:
        try:
            os.close(fd)
        except OSError as error:
            if error.errno != errno.EBADF:
                raise
    deny_outbound()
    os.execv(sys.argv[1], sys.argv[1:])


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError) as error:
        sys.exit(str(error))
