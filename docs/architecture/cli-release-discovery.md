# CLI release discovery

Kronn keeps the installed CLI version separate from the available stable release.
Release discovery is an asynchronous, in-memory snapshot with a six-hour TTL;
normal agent detection schedules stale refreshes and does not wait for network
I/O. A manual agent version check waits only for the bounded refresh path.
[src: file: backend/src/core/versions.rs:11-14]
[src: file: backend/src/core/versions.rs:84-115]

The discovery adapters query the official npm registry, PyPI JSON API, or a
project's GitHub latest-release endpoint. The snapshot retains a prior valid
release when a refresh fails, and exposes the source URL, check timestamp, and
error to agent detection. Tools with no configured verified source remain
explicitly unknown. [src: file: backend/src/core/versions.rs:27-54]
[src: file: backend/src/core/versions.rs:225-328]

RTK and ccusage are refreshed through the same snapshot. Discovery performs no
installation: RTK's upgrade command remains an explicit user action. The RTK
version endpoint separately reports the ccusage executable Kronn resolves for
usage reporting, its available stable version, check time, and error state.
[src: file: backend/src/core/versions.rs:15-17]
[src: file: backend/src/api/rtk.rs:412-486]
