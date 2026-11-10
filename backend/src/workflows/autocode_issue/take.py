import json
import re
import subprocess
import sys

# stdin: issue number, repo.
parts = sys.stdin.read().split('\n===SEP===\n')
parts += [''] * (2 - len(parts))


def clean(value):
    value = (value or '').strip()
    return '' if value.startswith(chr(123) * 2) else value


num, repo = clean(parts[0]), clean(parts[1]) or '__DEFAULT_REPO__'
if not num.isdigit():
    sys.stderr.write('Invalid issue number: %r\n' % num)
    sys.exit(1)
if not re.fullmatch(r'[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+', repo):
    sys.stderr.write('The repo variable must read owner/name, got %r.\n' % repo)
    sys.exit(1)
owner, name = repo.split('/')


def gh(*args, check=True):
    result = subprocess.run(['gh', 'api', *args], capture_output=True, text=True)
    if result.returncode != 0:
        if not check:
            return None
        sys.stderr.write('gh api %s: %s\n' % (args[0], (result.stderr or result.stdout)[-400:]))
        sys.exit(1)
    return result.stdout


issue = json.loads(gh('repos/%s/issues/%s' % (repo, num)))
if issue.get('pull_request'):
    sys.stderr.write('#%s is a pull request, not an issue.\n' % num)
    sys.exit(1)
if issue.get('state') != 'open':
    sys.stderr.write('#%s is closed.\n' % num)
    sys.exit(1)
assignees = [a['login'] for a in issue.get('assignees') or []]
me = json.loads(gh('user'))['login']
if assignees and me not in assignees:
    sys.stderr.write('#%s is already taken by %s.\n' % (num, ', '.join(assignees)))
    sys.exit(1)
# An open pull request that already closes the issue means someone is on it,
# even unassigned: never start a second implementation of the same issue.
query = ('query{repository(owner:"%s",name:"%s"){issue(number:%s){closedByPullRequestsReferences'
         '(first:10,includeClosedPrs:false){nodes{number author{login} headRefName}}}}}' % (owner, name, num))
nodes = json.loads(gh('graphql', '-f', 'query=' + query))['data']['repository']['issue']['closedByPullRequestsReferences']['nodes']
others = [p for p in nodes if not p['headRefName'].startswith('autocode/issue-')]
if others:
    sys.stderr.write('#%s already has an open pull request: %s. autoCode does not take it over.\n' % (
        num, ', '.join('#%d by @%s' % (p['number'], p['author']['login']) for p in others)))
    sys.exit(1)
if me not in assignees:
    # Only a collaborator may assign: a contributor without that right still
    # works the issue, so a refusal here is a note, not a stop.
    if gh('repos/%s/issues/%s' % (repo, num), '-X', 'PATCH', '-f', 'assignees[]=' + me, check=False) is None:
        sys.stderr.write('Could not assign #%s to @%s (no triage rights on %s): continuing without.\n' % (num, me, repo))
    else:
        sys.stderr.write('#%s assigned to @%s.\n' % (num, me))
sys.stdout.write(json.dumps({
    'number': issue['number'],
    'title': issue['title'],
    'body': issue.get('body') or '',
    'labels': [label['name'] for label in issue.get('labels') or []],
    'milestone': (issue.get('milestone') or {}).get('title'),
    'assignee': me,
}))
