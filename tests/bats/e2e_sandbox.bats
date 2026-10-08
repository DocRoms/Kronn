#!/usr/bin/env bats

# Floor, not an exact count: adding a case must not break this test, but losing
# cases (or the summary line) must.
E2E_SANDBOX_MIN_CASES=29

@test "publication sandbox refuses inherited authority and concurrent directory ownership" {
    run bash "$BATS_TEST_DIRNAME/../../scripts/tests/e2e-sandbox-backend.test.sh"
    [ "$status" -eq 0 ]
    summary="$(printf '%s\n' "$output" | grep -E '^[0-9]+ passed, [0-9]+ failed$' | tail -n 1)"
    [[ "$summary" =~ ^([0-9]+)\ passed,\ 0\ failed$ ]]
    [ "${BASH_REMATCH[1]}" -ge "$E2E_SANDBOX_MIN_CASES" ]
}

@test "publication E2E preflight refuses wrong ports, config and listener ownership" {
    run node --test "$BATS_TEST_DIRNAME/../../frontend/e2e/fixtures/publication-sandbox.test.mjs"
    [ "$status" -eq 0 ]
}
