FROM archlinux:latest AS runtime
RUN pacman -Syu --noconfirm --needed \
        binutils chromium ffmpeg grim mesa mesa-utils mpv nodejs noto-fonts perf \
        python sway ttf-dejavu vulkan-swrast vulkan-tools \
    && pacman -Scc --noconfirm
RUN useradd -m bench
WORKDIR /opt/native-tube

FROM runtime AS final
COPY . /opt/native-tube/
CMD ["/usr/bin/sleep", "infinity"]
RUN for binary in bin/*; do \
      ldd "$binary" > /tmp/nt-ldd 2>&1 || { cat /tmp/nt-ldd; exit 1; }; \
      if grep -E 'not found' /tmp/nt-ldd; then exit 1; fi; \
    done && rm /tmp/nt-ldd
