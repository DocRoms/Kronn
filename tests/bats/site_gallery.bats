#!/usr/bin/env bats

load test_helper

assert_file_contains() {
    grep -Fq -- "$2" "$1"
}

# The count was pinned at eight and went stale the first time the gallery grew.
# What matters is not how many cards there are, but that every card points at a
# file that exists and that the three languages show the same gallery — a
# screenshot added to one page and forgotten in the others is the real defect.
@test "public-site galleries reference checked-in assets, the same set in every language" {
    local page asset reference
    local -a assets
    reference=""
    for page in index.html en.html es.html; do
        mapfile -t assets < <(
            sed -nE 's#.*<a href="(screenshots/[^"?]+)" target="_blank".*#\1#p' \
                "$PROJECT_ROOT/site/$page" | sort
        )
        [ "${#assets[@]}" -ge 8 ]
        for asset in "${assets[@]}"; do
            [ -s "$PROJECT_ROOT/site/$asset" ]
        done
        if [ -z "$reference" ]; then
            reference="${assets[*]}"
        else
            [ "${assets[*]}" = "$reference" ]
        fi
    done
}

# A video card carries two sources; the fallback must be on disk too, or a
# browser that cannot play the first one shows an empty frame.
@test "public-site video cards ship both of their sources" {
    local page source
    local -a sources
    for page in index.html en.html es.html; do
        mapfile -t sources < <(
            sed -nE 's#.*<source src="(screenshots/[^"]+)".*#\1#p' \
                "$PROJECT_ROOT/site/$page"
        )
        for source in "${sources[@]}"; do
            [ -s "$PROJECT_ROOT/site/$source" ]
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
