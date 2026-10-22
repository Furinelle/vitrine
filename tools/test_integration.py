"""Run all HTTP regressions with disposable local D1/R2 and no production secrets."""
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import sys
import tempfile
import time
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
WRANGLER = ['node', str(ROOT / 'node_modules/wrangler/bin/wrangler.js')]


def main():
    # This project uses whole-line comments in wrangler.jsonc. Fail parsing if the
    # format changes, rather than silently testing a different configuration.
    source = '\n'.join(line for line in (ROOT / 'wrangler.jsonc').read_text().splitlines()
                       if not line.lstrip().startswith('//'))
    config = json.loads(source)
    subprocess.run(['cargo', 'build', '--bin', 'vitrine-similar'], cwd=ROOT, check=True)
    subprocess.run(['npm', 'run', 'build:dev'], cwd=ROOT, check=True)
    with tempfile.TemporaryDirectory(prefix='vitrine-integration-') as folder:
        temp = Path(folder)
        state = temp / 'state'
        # A separate config directory prevents loading the project's .dev.vars.
        test_config = {key: config[key] for key in
                       ('compatibility_date', 'assets', 'd1_databases', 'r2_buckets', 'ratelimits')}
        test_config.update(name='vitrine-integration', main=str(ROOT / config['main']), workers_dev=False,
                           vars={'INGEST_TOKEN': 'local-image-review-test', 'TELEGRAM_BOT_TOKEN': ''})
        test_config['assets']['directory'] = str(ROOT / config['assets']['directory'])
        for database in test_config['d1_databases']:
            database['migrations_dir'] = str(ROOT / database['migrations_dir'])
            database['remote'] = False
        for bucket in test_config['r2_buckets']:
            bucket['remote'] = False
        config_file = temp / 'wrangler.json'
        config_file.write_text(json.dumps(test_config))
        subprocess.run(WRANGLER + ['d1', 'migrations', 'apply', 'DB', '--local', '--config', str(config_file),
                                  '--persist-to', str(state)], cwd=ROOT, check=True, stdout=subprocess.DEVNULL)
        with socket.socket() as sock:
            sock.bind(('127.0.0.1', 0))
            port = sock.getsockname()[1]
        base = f'http://127.0.0.1:{port}'
        env = {**os.environ, 'VITRINE_TEST_BASE': base, 'VITRINE_TEST_STATE': str(state),
               'VITRINE_SIMILAR_BIN': str(ROOT / 'target/debug/vitrine-similar')}
        with (temp / 'worker.log').open('w+') as log:
            server = subprocess.Popen(WRANGLER + ['dev', '--local', '--config', str(config_file),
                                      '--ip', '127.0.0.1', '--port', str(port), '--inspector-port', '0',
                                      '--local-upstream', '127.0.0.1', '--upstream-protocol', 'http',
                                      '--persist-to', str(state)], cwd=ROOT, stdout=log, stderr=log,
                                      start_new_session=True)
            try:
                deadline = time.monotonic() + 30
                while time.monotonic() < deadline and server.poll() is None:
                    try:
                        with urllib.request.urlopen(base + '/api/health', timeout=1) as response:
                            if response.status == 200:
                                break
                    except (OSError, TimeoutError):
                        time.sleep(.2)
                else:
                    raise RuntimeError('Local Worker did not become ready')
                for test in ('test_public_security.py', 'test_public_browsing.py', 'test_admin_lifecycle.py',
                             'test_admin_security.py', 'test_image_review.py', 'test_catalog_cost.py',
                             'test_ingest_stream.py', 'test_tag_keys.py', 'test_tag_admin.py', 'test_curation.py'):
                    print(f'Running {test}', flush=True)
                    subprocess.run([sys.executable, str(ROOT / 'tools' / test)], cwd=ROOT, env=env, check=True)
            except BaseException:
                log.flush()
                log.seek(0)
                print(log.read()[-12000:], file=sys.stderr)
                raise
            finally:
                if server.poll() is None:
                    os.killpg(server.pid, signal.SIGTERM)
                try:
                    server.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    os.killpg(server.pid, signal.SIGKILL)
                    server.wait()
    print('PASS: all integration checks; disposable data removed; no production access')


if __name__ == '__main__':
    main()
