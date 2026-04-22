"""Kronn document-generation sidecar.

Launched by the Kronn Rust backend as a subprocess, listens on a loopback
port, converts HTML (or structured JSON) into the requested file format
and writes the result to a caller-supplied path. Kept stateless on
purpose: the Rust backend owns the file lifecycle + security checks.
"""

__version__ = "0.1.0"
