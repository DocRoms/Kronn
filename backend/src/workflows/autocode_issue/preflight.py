import json
import sys
import urllib.error
import urllib.parse
import urllib.request

# stdin: Kronn URL, repo, target branch, review rounds, issue number, reviewer,
# each on its own part. An empty or unresolved (raw template) part takes its default.
parts = sys.stdin.read().split('\n===SEP===\n')
parts += [''] * (6 - len(parts))


def clean(value):
    value = (value or '').strip()
    return '' if value.startswith(chr(123) * 2) else value


kronn, repo, branch, rounds, num, reviewer = [clean(part) for part in parts[:6]]
repo = repo or '__DEFAULT_REPO__'
branch = branch or '__DEFAULT_BRANCH__'
rounds = rounds or '__DEFAULT_ROUNDS__'
if not num.isdigit():
    sys.stderr.write('Invalid issue number: %r\n' % num)
    sys.exit(1)
if not rounds.isdigit() or int(rounds) < 1:
    sys.stderr.write('maxReviewRounds must be a whole number of at least 1, got %r.\n' % rounds)
    sys.exit(1)

query = urllib.parse.urlencode({
    'repo': repo,
    'target_branch': branch,
    'max_review_rounds': rounds,
    'branch': 'autocode/issue-' + num,
    'reviewer': reviewer,
})
try:
    with urllib.request.urlopen(kronn + '/api/contribution/preflight?' + query, timeout=60) as response:
        answer = json.loads(response.read().decode())
except (urllib.error.URLError, OSError, ValueError) as error:
    sys.stderr.write('Kronn did not answer the preflight at %s: %s\n' % (kronn, error))
    sys.exit(1)
if not answer.get('success'):
    sys.stderr.write('Preflight refused: %s\n' % (answer.get('error') or 'no message'))
    sys.exit(1)

report = answer['data']
for check in report['checks']:
    sys.stderr.write('[%s] %s: %s\n' % ('ok' if check['ok'] else 'FAIL', check['id'], check['message']))
if not report['ok']:
    sys.stderr.write('\nThis machine is not ready to contribute to %s. Fix the FAIL lines above, then run again.\n' % repo)
    sys.exit(1)
sys.stdout.write(json.dumps(report))
