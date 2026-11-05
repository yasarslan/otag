#!/usr/bin/env python3
"""Measure idle CPU and memory with disposable release-app fixtures.

Usage: python3 scripts/measure-idle.py --bundle target/Otag.app --tag after
Requires a macOS GUI login session. Never points fixtures at user sessions.
HOME, state, socket and preferences are disposable; persistence is enabled.
"""
import argparse, csv, json, os, pathlib, plistlib, re, shutil, signal, socket, statistics, subprocess, time

parser = argparse.ArgumentParser()
parser.add_argument('--bundle', required=True)
parser.add_argument('--tag', required=True)
parser.add_argument('--counts', default='0,2,4')
parser.add_argument('--hold', action='store_true')
args = parser.parse_args()
if not re.fullmatch(r'[A-Za-z0-9-]+', args.tag):
    parser.error('--tag must contain only letters, digits and hyphens')
counts = list(map(int, args.counts.split(',')))
if not counts or counts != sorted(set(counts)) or any(n not in (0, 2, 4) for n in counts):
    parser.error('--counts must be an increasing subset of 0,2,4')
root = pathlib.Path('/tmp') / ('otag-perf-' + args.tag)
root.mkdir(exist_ok=False)
bundle = root / 'Otag.app'
shutil.copytree(args.bundle, bundle)
info = bundle / 'Contents/Info.plist'
metadata = plistlib.loads(info.read_bytes())
domain = 'dev.otag.perf.' + args.tag
metadata['CFBundleIdentifier'] = domain
info.write_bytes(plistlib.dumps(metadata))
subprocess.run(['codesign', '--force', '--deep', '--sign', '-', str(bundle)], check=True, stdout=subprocess.DEVNULL)
subprocess.run(['defaults', 'write', domain, 'hasOnboarded', '-bool', 'true'], check=True)
subprocess.run(['defaults', 'write', domain, 'language', '-string', 'en'], check=True)
project = root / 'fixture'
project.mkdir(exist_ok=True)
sockpath = root / 'daemon.sock'
# Match the isolated test helpers: no inherited state overrides, real home,
# shell configuration or agent credentials. Parent-loss monitoring is a
# backstop if this measurement process is interrupted before normal cleanup.
home = root / 'home'
state = root / 'state'
home.mkdir()
environment = {
    'HOME': str(home),
    'OTAG_STATE_DIR': str(state),
    'XDG_STATE_HOME': str(home / 'xdg-state'),
    'XDG_RUNTIME_DIR': str(home / 'run'),
    'OTAG_SOCKET': str(sockpath),
    'OTAG_NO_NOTIFY': '1',
    'SHELL': '/bin/sh',
    'PATH': os.environ.get('PATH', '/usr/bin:/bin'),
}
log = (root / 'daemon.log').open('w')
daemon = subprocess.Popen([
    str(bundle / 'Contents/Resources/otag'), 'daemon', '--socket', str(sockpath),
    '--state', str(state), '--exit-with-parent', str(os.getpid()),
], env=environment, stdout=log, stderr=log)

def stop(process):
    if process.poll() is not None:
        return
    process.terminate()
    try:
        process.wait(timeout=15)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait()

def request(message):
    with socket.socket(socket.AF_UNIX) as conn:
        conn.settimeout(10)
        conn.connect(str(sockpath))
        conn.sendall(json.dumps(message).encode() + b'\n')
        data = b''
        while b'\n' not in data:
            chunk = conn.recv(65536)
            if not chunk:
                raise ConnectionError('fixture daemon closed before replying')
            data += chunk
        result = json.loads(data.split(b'\n')[0])
        if result['type'] == 'error': raise RuntimeError(result)
        return result

def interrupted(signum, frame):
    raise KeyboardInterrupt

signal.signal(signal.SIGTERM, interrupted)

ids = []
app = None
results = []
try:
    for _ in range(200):
        if daemon.poll() is not None:
            raise RuntimeError('daemon exited; see ' + str(root / 'daemon.log'))
        if sockpath.exists(): break
        time.sleep(.05)
    else: raise RuntimeError('daemon did not start')
    for count in counts:
        while len(ids) < count:
            response = request({'type':'start_session', 'cwd':str(project)})
            ids.append(response['pane'])
        if count:
            leaves = [{'pane':i} for i in ids]
            if count == 4:
                children = [{'horizontal':True,'children':leaves[:2],'fractions':[.5,.5]}, {'horizontal':True,'children':leaves[2:],'fractions':[.5,.5]}]
                tree = {'horizontal':False,'children':children,'fractions':[.5,.5]}
            else:
                tree = {'horizontal':True,'children':leaves,'fractions':[1/count]*count}
            layout = {'root':tree, 'focus':ids[0]}
        else:
            layout = {}
        subprocess.run(['defaults','write',domain,'windowLayout2','-data',json.dumps(layout).encode().hex()],check=True)
        appenv = dict(environment)
        app_log = (root / f'app-{count}.log').open('w')
        app = subprocess.Popen([str(bundle / 'Contents/MacOS/Otag')],env=appenv,stdout=app_log,stderr=app_log)
        print(json.dumps({'phase':'warming','sessions':count,'app':app.pid,'daemon':daemon.pid,'domain':domain}),flush=True)
        time.sleep(15)
        if args.hold:
            (root/'pids.json').write_text(json.dumps({'app':app.pid,'daemon':daemon.pid,'socket':str(sockpath),'domain':domain}))
            while not (root/'stop').exists(): time.sleep(1)
            break
        actual = request({'type': 'list_panes'})['panes']
        if len(actual) != count:
            raise RuntimeError(f'expected {count} fixture panes, got {len(actual)}')
        samples = []
        start = time.monotonic()
        with (root/f'{count}-samples.csv').open('w') as file:
            writer = csv.writer(file)
            writer.writerow(['elapsed_s','role','pid','cpu_percent','rss_kib'])
            for tick in range(61):
                if app.poll() is not None or daemon.poll() is not None: raise RuntimeError('fixture exited')
                output = subprocess.check_output(['ps','-p',f'{app.pid},{daemon.pid}','-o','pid=,%cpu=,rss='],text=True)
                if len(output.splitlines()) != 2:
                    raise RuntimeError('missing app or daemon in ps sample')
                for line in output.splitlines():
                    pid,cpu,rss = line.split()
                    role = 'app' if int(pid) == app.pid else 'daemon'
                    row = [round(time.monotonic()-start,3),role,int(pid),float(cpu),int(rss)]
                    writer.writerow(row)
                    samples.append(row)
                time.sleep(max(0, start+tick+1-time.monotonic()) if tick < 60 else 0)
        result = {'sessions':count,'samples':61,'duration_s':round(time.monotonic()-start,3),'persistence':True}
        for role in ['app','daemon']:
            rows = [r for r in samples if r[1]==role]
            result[role]={'cpu_mean':statistics.mean(r[3] for r in rows),'cpu_max':max(r[3] for r in rows),'rss_mean_mib':statistics.mean(r[4] for r in rows)/1024,'rss_max_mib':max(r[4] for r in rows)/1024}
        results.append(result)
        (root/'results.json').write_text(json.dumps(results,indent=2))
        print(json.dumps(result),flush=True)
        stop(app); app_log.close(); app=None
finally:
    if app: stop(app)
    try:
        if daemon.poll() is None:
            request({'type':'shutdown'})
            daemon.wait(timeout=15)
    except (OSError, RuntimeError, subprocess.TimeoutExpired):
        pass
    finally:
        stop(daemon)
        log.close()
        subprocess.run(['defaults', 'delete', domain], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    print('results:',root,flush=True)
