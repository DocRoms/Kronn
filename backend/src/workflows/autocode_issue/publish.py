import json
import subprocess
import sys
import time
import urllib.error
import urllib.request

# stdin: the preflight report, Kronn URL, repo, target branch, issue number.
parts = sys.stdin.read().split('\n===SEP===\n')
parts += [''] * (5 - len(parts))


def clean(value):
    value = (value or '').strip()
    return '' if value.startswith(chr(123) * 2) else value


def result_of(text):
    # `data_json` of an Exec step is its whole result; the payload is its stdout.
    data = json.loads(text) if text.strip() else {}
    if isinstance(data, dict) and 'stdout' in data and isinstance(data['stdout'], str):
        data = json.loads(data['stdout']) if data['stdout'].strip() else {}
    return data if isinstance(data, dict) else {}


report = result_of(parts[0])
kronn = clean(parts[1])
repo = clean(parts[2]) or '__DEFAULT_REPO__'
target = clean(parts[3]) or '__DEFAULT_BRANCH__'
num = clean(parts[4])
plan = report.get('publication')
project = report.get('project_id')
if not num.isdigit() or not plan or not project:
    sys.stderr.write('The preflight report holds no publication plan for issue %r.\n' % num)
    sys.exit(1)
branch = 'autocode/issue-' + num


def run(argv, check=True, cwd=None):
    result = subprocess.run(argv, capture_output=True, text=True, cwd=cwd)
    if check and result.returncode != 0:
        sys.stderr.write('%s: %s\n' % (' '.join(argv[:4]), (result.stderr or result.stdout)[-400:]))
        sys.exit(1)
    return result


try:
    with urllib.request.urlopen(kronn + '/api/projects/' + project, timeout=60) as response:
        depot = ((json.loads(response.read().decode()) or {}).get('data') or {}).get('path')
except (urllib.error.URLError, OSError, ValueError) as error:
    sys.stderr.write('Kronn did not answer: %s\n' % error)
    sys.exit(1)
if not depot:
    sys.stderr.write('Path of the %s clone not found.\n' % repo)
    sys.exit(1)


def git(*args, check=True):
    return run(['git', '-C', depot, *args], check=check)


def slug_of(url):
    url = url.strip().rstrip('/')
    url = url[:-4] if url.endswith('.git') else url
    return '/'.join(url.replace(':', '/').split('/')[-2:])


def remote_for(slug):
    for name in git('remote').stdout.split():
        if slug_of(git('remote', 'get-url', name).stdout).lower() == slug.lower():
            return name
    return None


if git('rev-parse', '--verify', '--quiet', 'refs/heads/' + branch, check=False).returncode != 0:
    sys.stderr.write('Branch %s does not exist in %s: run the autoCode Issue workflow for #%s first.\n' % (branch, depot, num))
    sys.exit(1)
base = remote_for(repo) or 'origin'
git('fetch', '--quiet', base, target)
ahead = git('rev-list', '--count', '%s/%s..%s' % (base, target, branch)).stdout.strip()
if ahead == '0':
    sys.stderr.write('%s holds no commit beyond %s/%s yet: wait for the campaign to deliver, then run this again.\n' % (branch, base, target))
    sys.exit(1)

title = run(['gh', 'api', 'repos/%s/issues/%s' % (repo, num), '--jq', '.title']).stdout.strip()
published = None
for step in plan['steps']:
    action = step['action']
    if action == 'fork_repo':
        sys.stderr.write('Forking %s.\n' % step['repo'])
        run(['gh', 'repo', 'fork', step['repo'], '--clone=false', '--remote=false'])
    elif action == 'push':
        name = remote_for(step['repo'])
        if not name:
            name = 'kronn-fork'
            git('remote', 'add', name, 'https://github.com/%s.git' % step['repo'])
        sys.stderr.write('Pushing %s to %s (%s).\n' % (step['branch'], step['repo'], name))
        # A fork just created may take a moment to accept a push.
        for attempt in range(6):
            pushed = git('push', name, '%s:%s' % (step['branch'], step['branch']), check=False)
            if pushed.returncode == 0:
                break
            if attempt == 5:
                sys.stderr.write('git push %s: %s\n' % (name, (pushed.stderr or pushed.stdout)[-400:]))
                sys.exit(1)
            time.sleep(5)
    elif action == 'open_pull_request':
        created = run(['gh', 'pr', 'create', '--repo', step['repo'], '--base', step['base'], '--head', step['head'],
                       '--title', 'Issue #%s - %s' % (num, title),
                       '--body', 'Closes #%s\n\nOpened by Kronn autoCode.' % num], check=False)
        text = (created.stdout + created.stderr).strip()
        if created.returncode != 0 and 'already exists' not in text:
            sys.stderr.write('gh pr create: %s\n' % text[-400:])
            sys.exit(1)
        published = text.split()[-1] if text else None
    else:
        sys.stderr.write('Unknown publication action %r.\n' % action)
        sys.exit(1)
sys.stdout.write(json.dumps({'issue': num, 'mode': plan['mode'], 'pull_request': published, 'branch': branch}))
