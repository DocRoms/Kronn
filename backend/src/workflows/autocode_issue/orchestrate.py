import json
import subprocess
import sys
import time
import urllib.error
import urllib.request

# stdin: the issue (take step), the preflight report, Kronn URL, repo, target
# branch, review rounds.
parts = sys.stdin.read().split('\n===SEP===\n')
parts += [''] * (6 - len(parts))


def clean(value):
    value = (value or '').strip()
    return '' if value.startswith(chr(123) * 2) else value


def result_of(text):
    # `data_json` of an Exec step is its whole result; the payload is its stdout.
    data = json.loads(text) if text.strip() else {}
    if isinstance(data, dict) and 'stdout' in data and isinstance(data['stdout'], str):
        data = json.loads(data['stdout']) if data['stdout'].strip() else {}
    return data if isinstance(data, dict) else {}


issue = result_of(parts[0])
report = result_of(parts[1])
kronn = clean(parts[2])
repo = clean(parts[3]) or '__DEFAULT_REPO__'
target = clean(parts[4]) or '__DEFAULT_BRANCH__'
rounds = clean(parts[5]) or '__DEFAULT_ROUNDS__'
num = str(issue.get('number') or '')
project = report.get('project_id')
if not num:
    sys.stderr.write('Missing issue.\n')
    sys.exit(1)
if not project:
    sys.stderr.write('The preflight did not resolve a Kronn project for %s.\n' % repo)
    sys.exit(1)
workers = report.get('workers') or []
if not workers:
    sys.stderr.write('The preflight found no local worker.\n')
    sys.exit(1)
worker = 'ClaudeCode' if 'ClaudeCode' in workers else workers[0]
github = 'https://github.com/' + repo


def call(path, method='GET', body=None, timeout=60):
    data = json.dumps(body).encode() if body is not None else None
    request = urllib.request.Request(kronn + path, data=data, method=method,
                                     headers={'Content-Type': 'application/json'} if data else {})
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            return json.loads(response.read().decode())
    except urllib.error.HTTPError as error:
        sys.stderr.write('%s %s: HTTP %s: %s\n' % (method, path, error.code, error.read().decode()[:400]))
        sys.exit(1)


def ok(answer):
    if isinstance(answer, dict) and answer.get('success') is False:
        sys.stderr.write('Kronn: %s\n' % (answer.get('error') or 'refused without a message'))
    return (answer or {}).get('data') if isinstance(answer, dict) else None


tag = 'issue-' + num


def tasks(label):
    page = ok(call('/api/planning/tasks?limit=100&tag=' + label)) or {}
    items = page.get('items') if isinstance(page, dict) else (page or [])
    return [t for t in (items or []) if t.get('status') not in ('archived', 'done')]


# 1) The task: the ticket already linked to the issue (tag github-N, full
#    brief and DoD), else an autoCode task issue-N (a rerun finds it again).
task = next(iter(tasks('github-' + num)), None)
if not task:
    task = next((t for t in tasks(tag) if 'autocode' in (t.get('tags') or [])), None)
generic_dod = [
    {'sentence': 'What the issue and the ticket describe is delivered, or the gap is justified.'},
    {'sentence': 'The relevant targeted tests pass; every behaviour change is covered by a test.'},
    {'sentence': 'Commits are DCO signed; the conventions of docs/AGENTS.md are followed.'},
]
if not task:
    task = ok(call('/api/planning/tasks', 'POST', {
        'title': ('Issue #%s - %s' % (num, issue.get('title', '')))[:200],
        'description': 'GitHub issue: %s/issues/%s\n\n## Request\n\n%s' % (github, num, issue.get('body') or '_(no description)_'),
        'project_ids': [project],
        'tags': [tag, 'autocode', 'github-' + num],
        'links': [{'label': 'GitHub #%s' % num, 'url': '%s/issues/%s' % (github, num)}],
        'status': 'todo',
        'definition_of_done': generic_dod,
    }))
if not task or not task.get('id'):
    sys.stderr.write('Task not found or not created.\n')
    sys.exit(1)
tid, ref = task['id'], task.get('reference')

# A ticket picked from the board may still be an idea without a definition of
# done: launching would be refused, so it becomes todo with the generic DoD.
current = ok(call('/api/planning/tasks/%s' % tid)) or {}
if current.get('status') != 'todo' or not current.get('definition_of_done'):
    update = {'status': 'todo', 'actor': {'kind': 'agent', 'id': worker}}
    if not current.get('definition_of_done'):
        update['definition_of_done'] = generic_dod
    if not ok(call('/api/planning/tasks/%s' % tid, 'PATCH', update)):
        sys.stderr.write('Task %s: moving it to todo was refused.\n' % ref)
        sys.exit(1)

# 2) The room: one this workflow already created for the issue (a rerun), else
#    a new one, linked to the task. Nobody answers in it: the campaign names
#    who reviews.
detail = ok(call('/api/planning/tasks/%s' % tid)) or {}
title = 'Issue #%s - autoCode' % num
room = None
for did in detail.get('discussion_ids') or []:
    disc = ok(call('/api/discussions/%s' % did)) or {}
    if (disc.get('title') or '').startswith(title):
        room = did
        break
if not room:
    created = ok(call('/api/discussions', 'POST', {
        'project_id': project,
        'title': title[:120],
        'agent': worker,
        'language': 'en',
        'no_agent': True,
        'initial_prompt': 'autoCode room of issue #%s: %s\n\nThe worker started by the campaign reports its progress here; arbitration happens from the Kronn UI.' % (num, issue.get('title', '')),
    }))
    room = (created or {}).get('id')
if not room:
    sys.stderr.write('Room creation refused.\n')
    sys.exit(1)
call('/api/planning/tasks/%s/discussions' % tid, 'POST', {'discussion_id': room})

# 3) The issue branch: cut from the freshly fetched target of the remote that
#    is the repository (never the local branch, which may carry an unpushed
#    integration), whatever that remote is called in this clone.
info = ok(call('/api/projects/%s' % project)) or {}
depot = info.get('path')
if not depot:
    sys.stderr.write('Path of the %s clone not found.\n' % repo)
    sys.exit(1)


def git(*args):
    result = subprocess.run(['git', '-C', depot, *args], capture_output=True, text=True)
    if result.returncode != 0:
        sys.stderr.write('git %s: %s\n' % (' '.join(args), (result.stderr or result.stdout)[-300:]))
        sys.exit(1)
    return result.stdout.strip()


def slug_of(url):
    url = url.strip().rstrip('/')
    url = url[:-4] if url.endswith('.git') else url
    return url.replace(':', '/').split('/')[-2:]


remote = 'origin'
for name in git('remote').split():
    if '/'.join(slug_of(git('remote', 'get-url', name))).lower() == repo.lower():
        remote = name
        break
git('fetch', '--quiet', remote, target)
branch = 'autocode/issue-%s' % num
exists = subprocess.run(['git', '-C', depot, 'rev-parse', '--verify', '--quiet', 'refs/heads/' + branch],
                        capture_output=True).returncode == 0
if not exists:
    git('branch', branch, '%s/%s' % (remote, target))

# 4) The campaign: the one active on this room, else a new one. The reviewer is
#    the one the preflight announced: the `reviewer` variable when set, else the
#    first remote model this instance has configured; with neither, the room as
#    it is (nobody answers in it: the human decides from the UI).
def run_of(view):
    if not isinstance(view, dict):
        return None
    return (view.get('run') or {}).get('id') or view.get('id')


campaign = ok(call('/api/orchestration/discussions/%s/campaign' % room))
run_id = run_of(campaign)
if not run_id:
    body = {
        'discussion_id': room,
        'project_id': project,
        'target_branch': branch,
        'default_worker': {'target': {'kind': 'agent', 'agent_type': worker}},
        'max_review_rounds': int(rounds),
    }
    if report.get('suggested_reviewer'):
        body['reviewer'] = report['suggested_reviewer']
    run_id = run_of(ok(call('/api/orchestration/campaigns', 'POST', body)))
if not run_id:
    sys.stderr.write('Campaign creation refused.\n')
    sys.exit(1)

# 5) Launch the task in the campaign (worker and reviews included).
launched = ok(call('/api/orchestration/campaigns/%s/launch' % run_id, 'POST', {
    'task_reference': ref,
    # One key per click (to the minute): a retry of the same click is
    # idempotent, a relaunch after a cancelled or finished execution gets a new
    # one instead of the old execution back.
    'idempotency_key': 'autocode-issue-%s-%s-%d' % (num, run_id, int(time.time() // 60)),
}))
execution = ((launched or {}).get('execution') or {}).get('id') or (launched or {}).get('id')
if not execution:
    sys.stderr.write('Launching %s in campaign %s was refused.\n' % (ref, run_id))
    sys.exit(1)
sys.stdout.write(json.dumps({
    'issue': num, 'task': ref, 'room': room, 'campaign': run_id,
    'execution': execution, 'branch': branch, 'target': target, 'repo': repo,
}))
