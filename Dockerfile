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

# Install OpenSSL - required for many Rust applications
RUN apt-get update && apt-get install -y openssl ca-certificates && rm -rf /var/lib/apt/lists/*

# Copy the binary from the builder stage
COPY --from=builder /usr/src/meshwatchy/target/release/mesh-watchy /usr/local/bin/mesh-watchy

# Set the startup command to run your binary
CMD ["mesh-watchy"]
