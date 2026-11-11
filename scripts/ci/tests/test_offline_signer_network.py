"""The real-prover test launcher must deny egress and retain inbound gRPC I/O."""

from pathlib import Path
import socket
import subprocess
import sys
import unittest


LAUNCHER = Path(__file__).resolve().parents[1] / "run-offline-signer.py"


class OfflineSignerNetworkTest(unittest.TestCase):
    def test_outbound_denied_and_inbound_tcp_remains_usable_after_exec(self):
        child_program = r'''
import errno, socket
for family in (socket.AF_INET, socket.AF_INET6):
    with socket.socket(family, socket.SOCK_STREAM) as sock:
        try:
            sock.connect(("127.0.0.1" if family == socket.AF_INET else "::1", 1))
        except OSError as error:
            assert error.errno == errno.EPERM, error
        else:
            raise AssertionError("outbound connect permitted")
for family, kind in ((socket.AF_INET, socket.SOCK_DGRAM), (socket.AF_INET, socket.SOCK_RAW),
                     (socket.AF_UNIX, socket.SOCK_STREAM)):
    try:
        socket.socket(family, kind)
    except OSError as error:
        assert error.errno == errno.EPERM, error
    else:
        raise AssertionError("non-TCP socket permitted")
for operation in ("sendto", "sendmsg"):
    with socket.socket() as sock:
        try:
            if operation == "sendto":
                sock.sendto(b"denied", socket.MSG_FASTOPEN, ("127.0.0.1", 1))
            else:
                sock.sendmsg([b"denied"], [], socket.MSG_FASTOPEN, ("127.0.0.1", 1))
        except OSError as error:
            assert error.errno == errno.EPERM, error
        else:
            raise AssertionError("Fast Open outbound connection permitted")
with socket.socket() as listener:
    listener.bind(("127.0.0.1", 0))
    listener.listen()
    print(listener.getsockname()[1], flush=True)
    conn, _ = listener.accept()
    with conn:
        assert conn.recv(32) == b"offline-request"
        conn.sendall(b"offline-response")
'''
        child = subprocess.Popen([sys.executable, str(LAUNCHER), sys.executable, "-c", child_program],
                                 stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        try:
            import select
            readable, _, _ = select.select([child.stdout], [], [], 10)
            self.assertTrue(readable, "filtered child did not become ready")
            line = child.stdout.readline().strip()
            if not line:
                _, stderr = child.communicate(timeout=10)
                self.fail(f"network filter failed before readiness: {stderr}")
            with socket.create_connection(("127.0.0.1", int(line)), timeout=10) as connection:
                connection.sendall(b"offline-request")
                self.assertEqual(connection.recv(32), b"offline-response")
            _, stderr = child.communicate(timeout=10)
            self.assertEqual(child.returncode, 0, stderr)
        finally:
            if child.poll() is None:
                child.kill()
                child.wait(timeout=10)
            child.stdout.close()
            child.stderr.close()

    def test_invalid_invocation_fails_closed(self):
        result = subprocess.run([sys.executable, str(LAUNCHER), "relative-program"],
                                capture_output=True, text=True, timeout=10)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("/absolute/executable", result.stderr)
