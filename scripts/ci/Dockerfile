FROM catthehacker/ubuntu:act-24.04@sha256:c58e2b364da03b0c804c7d660f2ecbedf2f221a382b9baa0b344b0144780ff43

USER root
SHELL ["/bin/bash", "-euo", "pipefail", "-c"]
RUN command -v dockerd && command -v docker && command -v sudo
RUN curl --fail --location --retry 3 https://github.com/nektos/act/releases/download/v0.2.89/act_Linux_x86_64.tar.gz -o /tmp/act.tar.gz \
    && echo '0191d6f1f3b716b5c55820032605d05fc3c1cdbf581ebeff655019e5dd1524c0  /tmp/act.tar.gz' | sha256sum --check \
    && tar -xzf /tmp/act.tar.gz -C /usr/local/bin act \
    && rm /tmp/act.tar.gz \
    && act --version | grep -Fx 'act version 0.2.89'

# act caches this executable before setup-node changes PATH to Node 22.
# https://github.com/nektos/act/blob/v0.2.89/pkg/runner/action.go
RUN curl --fail --location --retry 3 https://nodejs.org/dist/v24.22.1/node-v24.22.1-linux-x64.tar.xz -o /tmp/node.tar.xz \
    && echo '6c172a9ab6718f7b801fb5a7bd486af2ee5e32758086a04b84fcce2aa77095e6  /tmp/node.tar.xz' | sha256sum --check \
    && mkdir /opt/ci-node24 \
    && tar -xJf /tmp/node.tar.xz --strip-components=1 -C /opt/ci-node24 \
    && rm /tmp/node.tar.xz
ENV PATH=/opt/ci-node24/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin

RUN curl --fail --location --retry 3 https://dl.google.com/linux/chrome/deb/pool/main/g/google-chrome-stable/google-chrome-stable_155.0.8059.39-1_amd64.deb -o /tmp/chrome.deb \
    && echo 'c58aa0f2cd66179c9f050e062c882d27aa9b9f8c2b7c73fee3498560b5ed0b38  /tmp/chrome.deb' | sha256sum --check \
    && apt-get update \
    && apt-get install -y --no-install-recommends /tmp/chrome.deb \
    && rm -rf /tmp/chrome.deb /var/lib/apt/lists/* \
    && rm -f /etc/apt/sources.list.d/google-chrome.list \
    && chown root:root /opt/google/chrome/chrome-sandbox \
    && chmod 4755 /opt/google/chrome/chrome-sandbox
RUN groupadd --gid 10001 ci \
    && useradd --uid 10001 --gid ci --create-home --shell /bin/bash ci \
    && echo 'ci ALL=(ALL) NOPASSWD:ALL' > /etc/sudoers.d/ci \
    && chmod 0440 /etc/sudoers.d/ci \
    && mkdir -p /var/run/act /opt/hostedtoolcache \
    && chmod 0777 /var/run/act /opt/hostedtoolcache
ENV HOME=/home/ci USER=ci
WORKDIR /home/ci
