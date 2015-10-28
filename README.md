trawler-daemon-rust
===================

Daemon to throttle requests as part of trawler protocol

### Compilation

The daemon depends on zmq and rust-protobuf. 

```sh
$ sudo dnf install zeromq3 zeromq3-devel
```

The daemon is compiled using cargo.

```sh
$ cargo build daemon
```

### Running

The daemon does not self-daemonize. Use a terminal multiplexer, daemonizing script, init scripts, or even systemd to run it in the background.
