#!/bin/bash
set -e

URL="https://127.0.0.1:8082/?s=256k"
DURATION="30s"
CLIENTS=512
THREADS=8
STREAMS=8
COOLDOWN=30
LOADGEN="/datadisk/git-repos/learn-rust/http-bench/target/release/loadgen-rs"

echo "=== Benchmark Suite with ${COOLDOWN}s cooldown ==="
echo "URL: $URL | Duration: $DURATION | Clients: $CLIENTS | Threads: $THREADS | Streams: $STREAMS"
echo ""

# 1. h2load H1
echo ">>> [1/6] h2load HTTP/1.1"
podman run --rm --network host localhost/h2load -n 0 -D 30 -c $CLIENTS -t $THREADS --h1 $URL 2>&1 | tail -20
echo ""
echo "--- Cooldown ${COOLDOWN}s ---"
sleep $COOLDOWN

# 2. loadgen-rs H1
echo ">>> [2/6] loadgen-rs HTTP/1.1"
$LOADGEN --h1 -D $DURATION -c $CLIENTS -t $THREADS -m $STREAMS --insecure $URL 2>&1 | tail -30
echo ""
echo "--- Cooldown ${COOLDOWN}s ---"
sleep $COOLDOWN

# 3. h2load H2
echo ">>> [3/6] h2load HTTP/2"
podman run --rm --network host localhost/h2load -n 0 -D 30 -c $CLIENTS -t $THREADS -m $STREAMS --alpn-list=h2 $URL 2>&1 | tail -20
echo ""
echo "--- Cooldown ${COOLDOWN}s ---"
sleep $COOLDOWN

# 4. loadgen-rs H2
echo ">>> [4/6] loadgen-rs HTTP/2"
$LOADGEN --alpn-list h2 -D $DURATION -c $CLIENTS -t $THREADS -m $STREAMS --insecure $URL 2>&1 | tail -30
echo ""
echo "--- Cooldown ${COOLDOWN}s ---"
sleep $COOLDOWN

# 5. h2load H3
echo ">>> [5/6] h2load HTTP/3"
podman run --rm --network host localhost/h2load -n 0 -D 30 -c $CLIENTS -t $THREADS -m $STREAMS --alpn-list=h3 $URL 2>&1 | tail -20
echo ""
echo "--- Cooldown ${COOLDOWN}s ---"
sleep $COOLDOWN

# 6. loadgen-rs H3
echo ">>> [6/6] loadgen-rs HTTP/3"
$LOADGEN --alpn-list h3 -D $DURATION -c $CLIENTS -t $THREADS -m $STREAMS --insecure $URL 2>&1 | tail -30
echo ""

echo "=== All benchmarks complete ==="
