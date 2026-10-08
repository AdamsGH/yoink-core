"""Read-only /proc evidence collector, executed ONLY by docker exec, not worker.

No signals, process/state manufacture, permission changes or filesystem writes.
"""
import glob
import ipaddress
import json
import os
import pathlib
import re
import stat
import sys

RESOLVER = sys.argv[1]
assert ipaddress.ip_address(RESOLVER).version == 4
children = set()
for path in glob.glob('/proc/1/task/*/children'):
    children.update(int(pid) for pid in pathlib.Path(path).read_text().split())
workers, errors = [], []
for pid in sorted(children):
    try:
        root = pathlib.Path('/proc') / str(pid)
        argv = (root / 'cmdline').read_bytes().decode().rstrip('\0').split('\0')
        if '/opt/yoink-worker/main.py' not in argv:
            continue
        status = dict(line.split(':', 1) for line in (root / 'status').read_text().splitlines() if ':' in line)
        env = dict(part.split('=', 1) for part in (root / 'environ').read_bytes().decode().rstrip('\0').split('\0') if '=' in part)
        cwd = os.readlink(root / 'cwd')
        inodes = set()
        for fd in (root / 'fd').iterdir():
            try:
                match = re.fullmatch(r'socket:\[(\d+)\]', os.readlink(fd))
                if match:
                    inodes.add(match[1])
            except FileNotFoundError:
                pass
        sockets = []
        # These tables cover the shared network namespace, but only rows whose
        # inode belongs to this actual child's descriptors are accepted.
        for transport in ('udp', 'tcp'):
            for line in (root / 'net' / transport).read_text().splitlines()[1:]:
                fields = line.split()
                if len(fields) < 10 or fields[9] not in inodes:
                    continue
                local, remote = fields[1:3]
                def endpoint(value):
                    address, port = value.split(':')
                    address = str(ipaddress.ip_address(int(address, 16).to_bytes(4, sys.byteorder)))
                    return address, int(port, 16)
                local_ip, local_port = endpoint(local)
                remote_ip, remote_port = endpoint(remote)
                if (remote_ip == RESOLVER and remote_port == 53) or (transport == 'udp' and remote_ip == '0.0.0.0' and remote_port == 0):
                    sockets.append({'source': local_ip, 'port': local_port, 'transport': transport, 'inode': fields[9]})
        workers.append({'pid': pid, 'argv': argv, 'cwd': cwd, 'mode': stat.S_IMODE(os.stat(root / 'cwd').st_mode), 'parent': int(status['PPid']), 'uid': status['Uid'].strip().split(), 'noNewPrivs': status['NoNewPrivs'].strip(), 'capEff': status['CapEff'].strip(), 'privateOrigins': env.get('YOINK_WORKER_ALLOWED_PRIVATE_ORIGINS', '[]'), 'proxyOrigins': env.get('YOINK_WORKER_ALLOWED_PROXY_ORIGINS', '[]'), 'sockets': sockets})
    except FileNotFoundError:
        pass  # a raced exit is not live-worker evidence; caller must poll again
    except PermissionError as error:
        errors.append('read-only /proc observation denied: ' + str(error))
print(json.dumps({'children': sorted(children), 'workers': workers, 'errors': errors, 'resolver': pathlib.Path('/etc/resolv.conf').read_text()}))
