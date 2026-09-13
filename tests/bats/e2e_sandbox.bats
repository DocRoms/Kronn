#!/usr/bin/env bats

@test "publication sandbox refuses inherited authority and concurrent directory ownership" {
    run bash "$BATS_TEST_DIRNAME/../../scripts/tests/e2e-sandbox-backend.test.sh"
    [ "$status" -eq 0 ]
    [[ "$output" == *"27 passed, 0 failed"* ]]
}

@test "publication E2E preflight refuses wrong ports, config and listener ownership" {
    run node --test "$BATS_TEST_DIRNAME/../../frontend/e2e/fixtures/publication-sandbox.test.mjs"
    [ "$status" -eq 0 ]
}
