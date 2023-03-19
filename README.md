trawler-daemon-rust
===================

WARNING: Has not compiled in some time, needs repair work! Probably should strip out zmq and update the clients to match!

Daemon to throttle requests as part of trawler protocol

### Compilation

The daemon depends on zmq and rust-protobuf. 

```sh
$ sudo dnf install zeromq3 zeromq3-devel
```

The daemon is compiled using cargo.

```sh
$ cargo build
```

### Running

The daemon does not self-daemonize. Use a terminal multiplexer, daemonizing script, init scripts, or even systemd to run it in the background.
