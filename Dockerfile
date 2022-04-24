FROM ubuntu:20.04 as builder

## Install build dependencies.
RUN apt-get update && \
    DEBIAN_FRONTEND=noninteractive apt-get install -y cmake clang curl
RUN curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
RUN ${HOME}/.cargo/bin/rustup default nightly
RUN ${HOME}/.cargo/bin/cargo install -f cargo-fuzz

ADD . /repo
WORKDIR /repo

## TODO: ADD YOUR BUILD INSTRUCTIONS HERE.
# RUN ${HOME}/.cargo/bin/cargo build --all
RUN cd mles-utils/fuzz && ${HOME}/.cargo/bin/cargo fuzz build

# Package Stage
FROM ubuntu:20.04

# RUN ls /repo/mles-utils/

## TODO: Change <Path in Builder Stage>
COPY --from=builder repo/mles-utils/fuzz/target/x86_64-unknown-linux-gnu/release/mles_fuzz_decode_combined /
COPY --from=builder repo/mles-utils/fuzz/target/x86_64-unknown-linux-gnu/release/mles_fuzz_decode /
COPY --from=builder repo/mles-utils/fuzz/target/x86_64-unknown-linux-gnu/release/mles_fuzz_msg_decode /