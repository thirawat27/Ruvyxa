# Critical Server Reliability Plan

## Task Breakdown

- [x] Confirm the server's HTTP connection-information contract and identify the failure path.
  - Evidence: Axum 0.8.9 documents that `ConnectInfo` fails at runtime unless the router is served
    with `into_make_service_with_connect_info`; Ruvyxa's action endpoint extracts `ConnectInfo`.
- [x] Attach TCP peer metadata to every served request and add a regression test for the live HTTP
      path.
  - Done when: a request reaches a handler that extracts `ConnectInfo<SocketAddr>`.
- [x] Make forwarded client/protocol headers opt-in through exact trusted-proxy IPs, preserving
      loopback proxy support.
  - Done when: untrusted clients cannot spoof `X-Forwarded-For` or `X-Forwarded-Proto`, and the
    TypeScript config, Rust parser, runtime sanitizer, and documentation agree.
- [x] Verify narrow regression tests, workspace quality gates, package config tests, demo parity,
      and release metadata.
  - Done when: all relevant commands pass or a baseline failure is documented.
