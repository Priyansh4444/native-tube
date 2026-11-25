ARG BENCH_IMAGE=nt-fly-bench:local
FROM ${BENCH_IMAGE} AS builder
RUN pacman -Syu --noconfirm --needed gcc patch rustup && pacman -Scc --noconfirm
RUN rustup toolchain install 1.99.1 --profile minimal --component clippy --component rustfmt
WORKDIR /build
COPY tools/nt-input tools/nt-input
COPY tools/nt-explore tools/nt-explore
COPY tools/iso-test/sway.patch sway.patch
RUN patch -p1 < sway.patch
ENV CARGO_BUILD_JOBS=4
RUN cd tools/nt-input && cargo fmt --check && cargo clippy --locked --all-targets --all-features -- -D warnings && cargo test --locked && cargo build --locked --release
RUN cd tools/nt-explore && cargo fmt --check && cargo clippy --locked --all-targets --all-features -- -D warnings && cargo test --locked && cargo build --locked --release

FROM ${BENCH_IMAGE}
RUN pacman -Syu --noconfirm --needed wtype wl-clipboard vulkan-intel intel-media-driver && pacman -Scc --noconfirm
RUN if [ -n "$(getcap /usr/bin/sway)" ]; then setcap -r /usr/bin/sway; fi
COPY --from=builder /build/tools/nt-input/target/release/nt-input /usr/local/bin/nt-input
COPY --from=builder /build/tools/nt-explore/target/release/nt-explore /usr/local/bin/nt-explore
COPY tools/iso-test/start.sh tools/iso-test/batch.py tools/iso-test/perf-sway.py tools/iso-test/sway.conf /iso/
COPY tools/iso-test/explore-headless.sh /iso/tools/headless.sh
COPY tools/check-performance.py tools/perf-budget.json /iso/tools/
COPY tools/performance/ /iso/tools/performance/
COPY bench/shells.py /iso/bench/shells.py
WORKDIR /iso
RUN bash -n start.sh && bash -n tools/headless.sh && python -m py_compile batch.py perf-sway.py
ENTRYPOINT ["/iso/start.sh"]
