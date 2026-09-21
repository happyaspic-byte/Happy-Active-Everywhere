# Central management implementation plan

Use superpowers:executing-plans inline. Existing user authorization covers
implementation, commits and branch pushes. The user's testing instruction
overrides test-driven development gates: build/static checks only.

Spec: ../specs/2026-09-22-central-management-design.md

1. Preserve the previous path-identity correction and supersede automatic
   qualification with a build/package workflow.
2. Extract the local management operation executor. Add idempotent folder
   deployment, retaining existing identity, ACL and root validation.
3. Implement private registry/invitations, durable per-device command queues,
   pinned TLS agent exchange and durable agent result journal. Integrate the
   control worker with the managed service and add central/enroll CLI entrypoints.
4. Implement the central browser API and Korean fleet console: enrollment,
   device approval, folder pair deployment, remote inspection/actions and command
   results. Add persistent central-server configuration for the native service.
5. Document start/enroll/service/reverse-proxy flows and limitations. Build/lint,
   request one fresh static whole-change review, address material findings,
   commit/push and collect three-OS build/package results without running suites.

Ruling: optional server-location answer is absent; make the first deployment
usable on the current Mac with portable configuration and documented remote
browser access. No actual user device enrollment, service installation or user
folder assignment is performed without concrete device/path input.
