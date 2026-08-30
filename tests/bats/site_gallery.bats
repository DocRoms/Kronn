#!/usr/bin/env bats

load test_helper

assert_file_contains() {
    grep -Fq -- "$2" "$1"
}

@test "public-site galleries reference eight checked-in screenshots in every language" {
    local page screenshot
    local -a screenshots
    for page in index.html en.html es.html; do
        mapfile -t screenshots < <(
            sed -nE 's#.*<a href="(screenshots/[^"?]+)" target="_blank".*#\1#p' \
                "$PROJECT_ROOT/site/$page"
        )
        [ "${#screenshots[@]}" -eq 8 ]
        for screenshot in "${screenshots[@]}"; do
            [ -s "$PROJECT_ROOT/site/$screenshot" ]
        done
    done
}

@test "public-site lightbox keeps keyboard, focus and native new-tab affordances" {
    local page
    for page in index.html en.html es.html; do
        assert_file_contains "$PROJECT_ROOT/site/$page" \
            "overlay.setAttribute('aria-label', t.gallery)"
        assert_file_contains "$PROJECT_ROOT/site/$page" \
            "overlay.setAttribute('aria-hidden', 'true')"
        assert_file_contains "$PROJECT_ROOT/site/$page" \
            "else if (e.key === 'Tab')"
        assert_file_contains "$PROJECT_ROOT/site/$page" \
            "e.metaKey || e.ctrlKey || e.shiftKey || e.altKey"
        assert_file_contains "$PROJECT_ROOT/site/$page" \
            "aria-live=\"polite\""
    done
}

@test "public-site locales do not define the scanline overlay" {
    local page
    for page in index.html en.html es.html; do
        run grep -F "body::after {" "$PROJECT_ROOT/site/$page"
        assert_failure
    done
}

@test "public-site positioning covers deterministic workflows without overstating plugin counts" {
    assert_file_contains "$PROJECT_ROOT/site/index.html" \
        "agentiques ou entièrement déterministes"
    assert_file_contains "$PROJECT_ROOT/site/en.html" \
        "agentic or fully deterministic"
    assert_file_contains "$PROJECT_ROOT/site/es.html" \
        "agénticos o totalmente deterministas"

    run grep -E "20 (plugins )?(configurés|configured|configurados)" \
        "$PROJECT_ROOT/site/index.html" \
        "$PROJECT_ROOT/site/en.html" \
        "$PROJECT_ROOT/site/es.html"
    assert_failure
}
