trawler-daemon-c
================

Daemon to throttle requests as part of trawler protocol

### Compilation

The daemon depends on zmq, curl, and rust-protobuf. I.E:

```sh
$ sudo dnf install zeromq3 libcurl libcurl-devel
```

The daemon is compiled using cargo.

```sh
$ cargo build daemon
```

### Running

The daemon does not self-daemonize. Use a terminal multiplexer, daemonizing script, init scripts, or even systemd to run it in the background.
