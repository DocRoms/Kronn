"""KT-1367 bench: clef-flash:9b on 30 real Kronn cases through Ollama /v1/systemone.

Run with a local Ollama serving clef-flash:9b: python3 decision-step-bench-2026-10-21.py
"""
import json, urllib.request, time, sys
URL='http://localhost:11434/v1/systemone'
def ask(state, questions):
    body={"model":"clef-flash:9b","state":state,"questions":questions}
    req=urllib.request.Request(URL,data=json.dumps(body).encode(),headers={'Content-Type':'application/json'})
    t=time.time(); r=json.load(urllib.request.urlopen(req,timeout=300)); return r['answers'], time.time()-t

MERGE={"type":"noul","instructions":"Do the results show the change is ready to merge: every check passed and nothing is left to fix?"}
CONTINUE={"type":"noul","instructions":"Did the agent finish its task, so the workflow can go on to the next step?"}
ROUTE={"type":"choice","instructions":"Which model tier should do this task?","criteria":{"local":"trivial and mechanical: rename, format, move text, list files","economy":"routine work with a clear recipe: small fix, a test, docs update","advanced":"hard reasoning: architecture, security, concurrency, a bug nobody understands"}}
VERDICT={"type":"choice","instructions":"What does this review say about the change?","criteria":{"approve":"the reviewer accepts the change as it is","changes":"the reviewer asks for changes before accepting","unclear":"the text gives no verdict"}}
RISK={"type":"score","instructions":"How risky is shipping this change to users?","criteria":["no risk","low","medium","high","critical"]}

cases=[
 # (state, qid, question, expected)
 ("cargo test: test result: ok. 1104 passed; 0 failed; 3 ignored. clippy: 0 warnings. fmt: ok.","merge",MERGE,True),
 ("cargo test: test result: FAILED. 1101 passed; 3 failed. failures: api_tests::media_generate_reaches_the_local_comfyui","merge",MERGE,False),
 ("vitest: Test Files 513 passed. tsc: exit 0. eslint: 0 errors, 53 warnings (max 63).","merge",MERGE,True),
 ("tsc: src/components/Card.tsx(18,7): error TS2741: Property 'chosen' is missing. Found 4 errors.","merge",MERGE,False),
 ("All checks green. Reviewer: request_changes — 6 P2 to fix before integration.","merge",MERGE,False),
 ("Codex r9 APPROVE. 1186/1186 tests pass. Citations re-anchored, docs_citations ok.","merge",MERGE,True),
 ("clippy: error: this function has too many arguments (9/7) --> src/runner.rs:120. could not compile `kronn`.","merge",MERGE,False),
 ("make test: OK. make lint: OK. The PR has merge conflicts with main in 3 files.","merge",MERGE,False),
 ("Done: added the ComfyUI card, tests added and passing, committed 456aa2e13.","continue",CONTINUE,True),
 ("I could not finish: the build fails with a linker error and I do not know how to fix it.","continue",CONTINUE,False),
 ("I started on the migration but ran out of context halfway; three files remain untouched.","continue",CONTINUE,False),
 ("Task complete. All 6 acceptance criteria are met; see the summary above.","continue",CONTINUE,True),
 ("Here is my plan: 1. read the code 2. write the fix 3. run tests. I will start now.","continue",CONTINUE,False),
 ("[SIGNAL: OK] Report written to docs/reports/audit.md, 12 findings, all with citations.","continue",CONTINUE,True),
 ("Rename the variable `cfg` to `config` in src/lib.rs.","route",ROUTE,"local"),
 ("Reformat the CHANGELOG entries to the house style.","route",ROUTE,"local"),
 ("Add a unit test for parse_history when the output list is empty.","route",ROUTE,"economy"),
 ("Fix the typo in the French translation of the ComfyUI card title.","route",ROUTE,"local"),
 ("Find why two workers sometimes bill the same media job twice under load; the race is not reproducible.","route",ROUTE,"advanced"),
 ("Design the confinement of downloads against symlink and hardlink attacks across macOS, Linux and Windows.","route",ROUTE,"advanced"),
 ("Update the README section about Ollama with the new model list.","route",ROUTE,"economy"),
 ("LGTM, ship it. Nice tests.","verdict",VERDICT,"approve"),
 ("Two blocking issues: the lock is not released on error, and the test only covers the happy path. Please fix.","verdict",VERDICT,"changes"),
 ("I looked at the diff while waiting for CI.","verdict",VERDICT,"unclear"),
 ("Approved after the second round; the remaining nit is optional.","verdict",VERDICT,"approve"),
 ("request_changes: P2 security — .part opened by path, symlink writes outside install_dir.","verdict",VERDICT,"changes"),
 ("Change: fix a typo in a code comment.","risk",RISK,(0,1)),
 ("Change: rewrite the credential encryption and the key rotation of every stored secret.","risk",RISK,(3,4)),
 ("Change: add an optional field to a settings card, default off, with tests.","risk",RISK,(0,1)),
 ("Change: drop and recreate the media_jobs table during migration, no backup.","risk",RISK,(3,4)),
]
rows=[]
cold=None
for state,qid,q,exp in cases:
    a,dt=ask(state,{qid:q})
    if cold is None: cold=dt
    ans=a[qid]
    if q["type"]=="noul":
        p=ans["noul"]; got=p>=0.5; conf=abs(2*p-1); ok=(got==exp); shown=f"p={p:.3f}"
    elif q["type"]=="choice":
        got=ans["choice"]; conf=ans["confidence"]; ok=(got==exp); shown=got
    else:
        probs=ans["probabilities"]; got=int(max(probs,key=lambda k:probs[k])); conf=ans["confidence"]; ok=got in exp; shown=f"argmax={got} score={ans['score']:.2f}"
    rows.append((qid,ok,conf,shown,round(dt,2)))
    print(f"{qid:9} ok={ok!s:5} conf={conf:.3f} {shown}  {dt:.2f}s")
print("cold first call", round(cold,2),"s")
print("accuracy", sum(r[1] for r in rows),"/",len(rows))
for t in [0.0,0.1,0.2,0.3,0.4,0.5,0.6,0.7]:
    kept=[r for r in rows if r[2]>=t]
    good=sum(r[1] for r in kept)
    print(f"threshold {t:.1f}: kept {len(kept)}/{len(rows)}, correct among kept {good}/{len(kept)}, escalated {len(rows)-len(kept)}")
