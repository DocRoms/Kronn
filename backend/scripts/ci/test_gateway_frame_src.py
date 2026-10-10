#!/usr/bin/env python3
"""The Docker gateway puts the allowed embed sites in the app document's
frame-src, and keeps one CSP and every security header doing it. The desktop
and native (Vite) servers put the same host frame policy on their documents."""

from __future__ import annotations

import pathlib
import re
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[3]
CONF = (ROOT / ".docker" / "nginx.conf").read_text()
LIVE_PAGES = (ROOT / "backend" / "src" / "api" / "live_pages.rs").read_text()
DESKTOP = (ROOT / "desktop" / "src-tauri" / "src" / "main.rs").read_text()
VITE = (ROOT / "frontend" / "vite.config.ts").read_text()


def strip_comments(conf: str) -> str:
    return re.sub(r"#[^\n]*", "", conf)


def blocks(conf: str) -> dict[str, str]:
    """Body of each `location` block, keyed by its head."""
    found = {}
    for match in re.finditer(r"location\s+([^{]+?)\s*\{", conf):
        depth, start = 1, match.end()
        index = start
        while depth:
            depth += {"{": 1, "}": -1}.get(conf[index], 0)
            index += 1
        found[match.group(1).strip()] = conf[start : index - 1]
    return found


class GatewayFrameSrcTest(unittest.TestCase):
    def setUp(self):
        self.conf = strip_comments(CONF)
        self.locations = blocks(self.conf)

    def test_one_csp_with_the_dynamic_frame_src(self):
        csp = re.findall(r'add_header\s+Content-Security-Policy\s+"([^"]*)"', self.conf)
        self.assertEqual(len(csp), 1)
        self.assertIn("frame-src $kronn_frame_src;", csp[0])
        self.assertNotIn("*", csp[0].split("frame-src", 1)[1])
        self.assertIn("set $kronn_frame_src \"'self'\";", self.conf)

    def test_locations_never_drop_the_server_headers(self):
        for header in (
            "X-Frame-Options",
            "X-Content-Type-Options",
            "X-XSS-Protection",
            "Referrer-Policy",
        ):
            self.assertRegex(self.conf, rf"add_header\s+{header}\s+\"[^\"]+\"\s+always;")
        for head, body in self.locations.items():
            self.assertNotIn("add_header", body, head)

    def test_document_location_reads_the_backend_list(self):
        document = self.locations["/"]
        self.assertIn("auth_request /_kronn/frame-src;", document)
        self.assertIn(
            "auth_request_set $kronn_frame_src $upstream_http_x_kronn_frame_src;", document
        )
        self.assertIn("error_page 500 = @frontend_self_frames;", document)
        fallback = self.locations["@frontend_self_frames"]
        self.assertIn("set $kronn_frame_src \"'self'\";", fallback)
        for body in (document, fallback):
            for directive in (
                "proxy_hide_header Content-Security-Policy;",
                "proxy_hide_header ETag;",
                "proxy_hide_header Last-Modified;",
                'proxy_set_header If-None-Match "";',
                'proxy_set_header If-Modified-Since "";',
            ):
                self.assertIn(directive, body)
        self.assertNotIn("auth_request", self.locations["/assets/"])

    def test_documents_carry_the_served_sources_marker(self):
        marker = (
            "sub_filter '<head>' '<head><meta name=\"kronn-served-frame-src\" "
            "content=\"$kronn_frame_src\">';"
        )
        for head in ("/", "@frontend_self_frames"):
            body = self.locations[head]
            self.assertIn(marker, body, head)
            self.assertIn("sub_filter_once on;", body, head)
            # sub_filter cannot rewrite a compressed upstream body.
            self.assertIn('proxy_set_header Accept-Encoding "";', body, head)

    def test_lookup_targets_the_backend_route(self):
        path = re.search(r'EMBED_FRAME_SRC_PATH: &str = "([^"]+)"', LIVE_PAGES).group(1)
        lookup = self.locations["= /_kronn/frame-src"]
        self.assertIn("internal;", lookup)
        self.assertIn(f"proxy_pass http://backend:3140{path};", lookup)


class HostFramePolicyWiringTest(unittest.TestCase):
    def test_desktop_serves_its_documents_with_the_policy(self):
        # With a null Tauri CSP, this response header is the only enforcement.
        self.assertIn("kronn::api::live_pages::serve_app_documents(", DESKTOP)
        self.assertNotIn("ServeDir::new", DESKTOP)

    def test_native_dev_server_serves_its_documents_with_the_policy(self):
        self.assertRegex(VITE, r"plugins:\s*\[[^\]]*framePolicyPlugin\(backendTarget\)")


if __name__ == "__main__":
    unittest.main()
