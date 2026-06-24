FROM debian:stable

# Install packaging and build dependencies
RUN apt-get update && apt-get install -y \
    debhelper \
    fakeroot \
    dpkg-dev \
    curl \
    ca-certificates \
    git \
    build-essential \
    && rm -rf /var/lib/apt/lists/*

# Set up build directory
WORKDIR /build/pve-san-fenced

# Copy source tree
COPY . .

# Set up entrypoint script to run build and copy outputs
COPY docker-entrypoint.sh /docker-entrypoint.sh
RUN chmod +x /docker-entrypoint.sh

ENTRYPOINT ["/docker-entrypoint.sh"]
