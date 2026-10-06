#!/usr/bin/env python3
"""Tests for the nginx/axum body limit check (KT-1064)."""

from __future__ import annotations

import importlib.util
import pathlib
import unittest

_SPEC = importlib.util.spec_from_file_location(
    "nginx_body_limits", pathlib.Path(__file__).with_name("nginx_body_limits.py")
)
nginx_body_limits = importlib.util.module_from_spec(_SPEC)
_SPEC.loader.exec_module(nginx_body_limits)

LIB = """
        .route(
            "/api/config/import",
            post(api::setup::import_data)
                .layer(axum::extract::DefaultBodyLimit::max(512 * 1024 * 1024)),
        )
        .route("/api/health", get(health))
        .route(
            "/api/docs/pdf",
            post(api::docs::generate_pdf)
                .layer(axum::extract::DefaultBodyLimit::max(64 * 1024 * 1024)),
        )
"""

CONF_OK = """
server {
    # A comment naming a location { must not hide the next directive.
    client_max_body_size 64m;
    location = /api/config/import {
        client_max_body_size 512m;
        proxy_pass http://backend:3140;
    }
    location /api/ { proxy_pass http://backend:3140; }
}
"""


class NginxBodyLimitsTest(unittest.TestCase):
    def test_parses_route_limits_only_where_set(self):
        self.assertEqual(
            nginx_body_limits.axum_limits(LIB),
            {"/api/config/import": 512 * 1024**2, "/api/docs/pdf": 64 * 1024**2},
        )

    def test_exact_location_covers_a_larger_route(self):
        self.assertEqual(nginx_body_limits.violations(LIB, CONF_OK), [])

    def test_missing_exact_location_is_reported(self):
        conf = "server {\n    client_max_body_size 64m;\n}\n"
        found = nginx_body_limits.violations(LIB, conf)
        self.assertEqual(len(found), 1)
        self.assertIn("/api/config/import", found[0])

    def test_server_cap_below_a_route_is_reported(self):
        conf = CONF_OK.replace("client_max_body_size 64m;", "client_max_body_size 32m;")
        found = nginx_body_limits.violations(LIB, conf)
        self.assertEqual(len(found), 1)
        self.assertIn("/api/docs/pdf", found[0])

    def test_unset_server_cap_falls_back_to_the_nginx_default(self):
        server, exact = nginx_body_limits.nginx_limits("server { listen 80; }")
        self.assertEqual((server, exact), (1024**2, {}))

    def test_repository_files_are_consistent(self):
        self.assertEqual(nginx_body_limits.main(), 0)


if __name__ == "__main__":
    unittest.main()
