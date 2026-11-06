FROM rclone/rclone:1.75.2 AS rclone

FROM ubuntu:24.04

RUN apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=rclone /usr/local/bin/rclone /usr/local/bin/rclone
COPY scripts/session.sh /usr/local/bin/icloud-session

RUN groupadd --gid 10001 icloud \
    && useradd --uid 10001 --gid 10001 --create-home --shell /bin/bash icloud \
    && install -d -o icloud -g icloud -m 0700 /state /state/downloads \
    && chmod 0755 /usr/local/bin/icloud-session

ENV RCLONE_CONFIG=/state/rclone.conf

USER icloud
WORKDIR /state/downloads
RUN bash -n /usr/local/bin/icloud-session && rclone version \
    && test_state="$(mktemp -d)" \
    && export RCLONE_CONFIG="$test_state/rclone.conf" RCLONE_CONFIG_PASS="local-test-fixture" \
    && rclone --ask-password=false config encryption set \
        --password-command 'printenv RCLONE_CONFIG_PASS' >/dev/null \
    && rclone --ask-password=false config create icloud local --no-output \
    && mkdir "$test_state/downloads" \
    && cd "$test_state/downloads" \
    && touch sample-file.txt \
    && printf '%s\n' "$RCLONE_CONFIG_PASS" | /usr/local/bin/icloud-session > "$test_state/output" \
    && grep -qx 'sample-file.txt' "$test_state/output" \
    && grep -q 'Access verified' "$test_state/output" \
    && cd /state/downloads \
    && rm -rf "$test_state"
CMD ["/usr/local/bin/icloud-session"]
