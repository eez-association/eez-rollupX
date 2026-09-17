import json, pathlib, sys, time
root = int(sys.argv[1])
out = pathlib.Path(sys.argv[2])
def stat(pid):
    try:
        raw = pathlib.Path(f'/proc/{pid}/stat').read_text()
        fields = raw[raw.rfind(')') + 2:].split()
        return int(fields[1]), fields[19]
    except (OSError, ValueError, IndexError):
        return None
initial = stat(root)
if initial is None:
    raise SystemExit('native test process is not alive')
peaks = {}
with out.open('w') as log:
    while stat(root) == initial:
        processes = {}
        for p in pathlib.Path('/proc').iterdir():
            if p.name.isdecimal():
                value = stat(int(p.name))
                if value is not None:
                    processes[int(p.name)] = value
        owned = {root}
        while True:
            extra = {pid for pid, (parent, _) in processes.items() if parent in owned} - owned
            if not extra:
                break
            owned.update(extra)
        sample = {'time_unix': time.time(), 'processes': []}
        for pid in sorted(owned):
            try:
                status = {}
                for line in pathlib.Path(f'/proc/{pid}/status').read_text().splitlines():
                    key, _, value = line.partition(':')
                    if key in ('Name', 'VmRSS', 'VmHWM', 'VmSwap'):
                        status[key] = value.strip()
                if pid != root and not status.get('Name', '').startswith('eez-'):
                    continue
                io = {}
                for line in pathlib.Path(f'/proc/{pid}/io').read_text().splitlines():
                    key, _, value = line.partition(':')
                    if key in ('read_bytes', 'write_bytes'):
                        io[key] = int(value)
                record = {'pid': pid, 'start_ticks': processes[pid][1], **status, **io}
                sample['processes'].append(record)
                identity = str(pid) + ':' + processes[pid][1]
                old = peaks.get(identity)
                if old is None or int(status.get('VmHWM', '0').split()[0]) >= int(old.get('VmHWM', '0').split()[0]):
                    peaks[identity] = record
            except (OSError, KeyError, ValueError):
                continue
        log.write(json.dumps(sample, separators=(',', ':')) + '\n')
        log.flush()
        out.with_suffix('.peaks.json').write_text(json.dumps(peaks, indent=2) + '\n')
        time.sleep(2)
print('native process exited; resource samples complete')
