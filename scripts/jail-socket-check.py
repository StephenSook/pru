"""Run inside pru-agent-jail: every socket except Unix sockets and TCP streams must be refused.

The container smoke pipes this file into the jail as uid 10000. It exits non-zero and names each
socket the filter let through.
"""

import ctypes
import errno
import platform
import socket

X32_SYSCALL_BIT = 0x40000000
SYS_SOCKET_X86_64 = 41


def refused(make):
    try:
        make().close()
    except OSError as error:
        return error.errno == errno.EPERM
    return False


checks = {
    "IPv4 UDP": lambda: socket.socket(socket.AF_INET, socket.SOCK_DGRAM),
    "IPv6 UDP": lambda: socket.socket(socket.AF_INET6, socket.SOCK_DGRAM),
    "IPv4 UDP, non-blocking": lambda: socket.socket(socket.AF_INET, socket.SOCK_DGRAM | socket.SOCK_NONBLOCK),
    "IPv4 raw": lambda: socket.socket(socket.AF_INET, socket.SOCK_RAW, socket.IPPROTO_ICMP),
    "IPv4 SCTP stream": lambda: socket.socket(socket.AF_INET, socket.SOCK_STREAM, 132),
    "IPv6 SCTP stream": lambda: socket.socket(socket.AF_INET6, socket.SOCK_STREAM, 132),
    "IPv4 MPTCP stream": lambda: socket.socket(socket.AF_INET, socket.SOCK_STREAM, 262),
    "netlink": lambda: socket.socket(socket.AF_NETLINK, socket.SOCK_RAW, 0),
    "packet": lambda: socket.socket(socket.AF_PACKET, socket.SOCK_RAW, 0),
}
failed = [name for name, make in checks.items() if not refused(make)]

if platform.machine() == "x86_64":
    libc = ctypes.CDLL(None, use_errno=True)
    libc.syscall.restype = ctypes.c_long
    fd = libc.syscall(
        ctypes.c_long(X32_SYSCALL_BIT | SYS_SOCKET_X86_64), socket.AF_INET, socket.SOCK_DGRAM, 0
    )
    if fd != -1 or ctypes.get_errno() != errno.EPERM:
        failed.append(f"IPv4 UDP through the x32 syscall number (fd {fd}, errno {ctypes.get_errno()})")

# What Hermes needs still works: Unix socket pairs and TCP streams.
for end in socket.socketpair():
    end.close()
socket.socket(socket.AF_INET, socket.SOCK_STREAM).close()
socket.socket(socket.AF_INET, socket.SOCK_STREAM, socket.IPPROTO_TCP).close()

if failed:
    raise SystemExit("not refused: " + ", ".join(failed))
print("jail-socket-check ok")
