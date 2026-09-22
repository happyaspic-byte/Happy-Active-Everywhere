# Central web management

The product owner requests Resilio-style web operation and central management of
multiple devices. The earlier elapsed-time and performance qualification is no
longer an implementation gate; the owner will perform acceptance testing.

## Product flow

One server presents a Korean fleet console. The operator creates a one-device,
24-hour invitation, imports it on a device once, and approves its displayed
fingerprint in the console. The console shows heartbeat age, folders, running
jobs, last successful sync, errors and durable command outcomes. A two-device
folder wizard distributes mutual trust, folder permissions and paired jobs.
Folder paths belong to each device; existing content is preserved. More pairs
can connect a third device to the same folder. Pause/resume, deletion review,
conflict resolution and history restore use existing engine operations.

## Architecture and authorization

- Rust/Axum server with a private SQLite registry and command queue. Embedded
  static assets; no Node runtime or third-party web resources.
- Separate outbound agent control connection using TLS 1.3, a pinned server
  certificate and a per-device random credential. Client TLS proves possession
  of the enrolled device's private key; the application binds every request to
  that handshake identity. Invitations are single-use,
  expiring and never placed in URLs. Pending/revoked agents cannot fetch work.
- Browser API uses the existing private bearer login, exact Host/Origin checks
  and CSP. Default listener is loopback. Remote browser access uses an SSH tunnel
  or an HTTPS reverse proxy with an explicitly configured public origin; direct
  unencrypted external management listeners are rejected.
- Agents run inside the existing managed service and share its job supervisor.
  No remote shell. A typed command enum delegates to existing folder/ACL/jobs
  operations. File data remains peer-to-peer with existing mutual TLS and ACLs.
- Server commands survive restarts; agents keep a durable execution journal.
  Completed command IDs are acknowledged without re-execution. After an agent
  dies during an operation, report an uncertain outcome and require inspection;
  do not blindly replay a possibly applied deletion or restore.
- Folder deployment is an idempotent typed configuration command. Intermediate
  failures remain visible; retry may finish matching configuration but cannot
  silently change an existing folder root/mode or another job's binding.
- Offline devices retain sync configuration. Revocation prevents central work;
  file-peer permission withdrawal is a separate queued command whose completion
  must be observed. No claim that an offline device is immediately controlled.

## Scope and limits

First deployment defaults to the current Mac. The executable and native user
service support Mac/Linux/Windows. Central server state is separate from agent
state. LAN/VPN IP:port reachability is supplied when connecting folders. NAT
relay, on-demand files, enterprise SSO and cluster availability are later work.
The console reports observed process/status information, never invented transfer
rates or unobserved successful synchronization.

## Verification boundary

Run compilation, formatting, lint and package generation only. Preserve existing
test scripts and evidence for the operator; do not run new behavioral suites,
long-duration tests, destructive volume probes or benchmarks. A fresh static
whole-change review checks authentication, replay and data preservation.
