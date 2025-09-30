# Use the official Rust image as a parent image
FROM rust:1.90 as builder

# Set the working directory in the container
WORKDIR /usr/src/meshwatchy

# for paho
RUN apt-get update && apt-get install -y cmake npm --no-install-recommends

# Copy the entire project
COPY . .

# build the FE bullcrap
RUN cd assets && mkdir -p dist/css && npm ci &&  npm run build:all

# set some env shit for fuckery
ENV CARGO_MANIFEST_DIR=/usr/src/meshwatchy

# Build the project
RUN cargo build --release

# Start a new stage with a minimal image
FROM debian:trixie-slim

# Install OpenSSL and SQLite3 - required for many Rust applications
RUN apt-get update && apt-get install -y openssl ca-certificates sqlite3 libsqlite3-dev && rm -rf /var/lib/apt/lists/*

# Create application directory and data directory
WORKDIR /app

# Create a data directory for the database with proper permissions
RUN mkdir -p /app/data && chmod 755 /app/data

# Copy the binary from the builder stage
COPY --from=builder /usr/src/meshwatchy/target/release/mesh-watchy /usr/local/bin/mesh-watchy

# Copy required runtime files
COPY --from=builder /usr/src/meshwatchy/config.container.toml /app/config.toml
COPY --from=builder /usr/src/meshwatchy/static /app/static/
COPY --from=builder /usr/src/meshwatchy/templates /app/templates/
COPY --from=builder /usr/src/meshwatchy/assets/dist /app/assets/dist/
COPY --from=builder /usr/src/meshwatchy/webhooks.toml /app/

# Create a volume for persistent data
VOLUME ["/app/data"]

# Set the startup command to run your binary
CMD ["mesh-watchy"]
