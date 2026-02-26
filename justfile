# R4L-MKernel Build System

build_dir := "./build"
module_dir := "./rkernel/rm-drivers"
test_dir := "./rkernel/test"
flags := "O=" + build_dir + " LLVM=1 CLIPPY=1 -j$(nproc)"
bzImage := build_dir + "/arch/x86/boot/bzImage"
rs_dirs := "rust/kernel/rkernel rkernel/rm-drivers rkernel/test"
fmt_config := "rust/kernel/rkernel/.rustfmt.toml"

default:
    @just --list

# Build

build: _fmt _lint _config
    make {{ flags }}
    make {{ flags }} M={{ test_dir }}/domain_test
    make {{ flags }} M={{ test_dir }}/objectspace_test
    make {{ flags }} M={{ test_dir }}/rarc_test
    make {{ flags }} M={{ test_dir }}/nullcap_test
    make {{ flags }} M={{ test_dir }}/pci_cap_test
    make {{ flags }} M={{ test_dir }}/domain_module_test
    make {{ flags }} M={{ test_dir }}/panicked_thread_test
    make {{ flags }} M={{ test_dir }}/nested_call_test
    make {{ flags }} M={{ test_dir }}/revocable_test
    make {{ flags }} M={{ module_dir }}/rnvme_domain

# Run (virtme-ng)

vng cmd="":
    #!/usr/bin/env bash
    # Pin to bench cgroup (CPU 24-47, NUMA node 1) if available
    [[ -d /sys/fs/cgroup/bench ]] && echo $$ > /sys/fs/cgroup/bench/cgroup.procs 2>/dev/null
    args=(--run {{ bzImage }} --memory 4G --rw)
    qemu="-device pci-testdev"
    (exec 3>/dev/vfio/18) 2>/dev/null && qemu+=" -device vfio-pci,host=87:00.0"
    args+=(--qemu-opts="$qemu")
    [[ -n "{{ cmd }}" ]] && args+=(--exec "{{ cmd }}")
    virtme-ng "${args[@]}"

vng-emu cmd="" size="4096":
    #!/usr/bin/env bash
    img="rkernel/.tmp/nvme-bench.img"
    mkdir -p "$(dirname "$img")"
    [[ ! -f "$img" || $(stat -c%s "$img") -ne $(({{ size }} * 1048576)) ]] && \
        dd if=/dev/zero of="$img" bs=1M count={{ size }} 2>/dev/null
    args=(--run {{ bzImage }} --memory 4G --rw)
    qemu="-device pci-testdev"
    qemu+=" -drive file=$img,if=none,id=nvme0,format=raw -device nvme,drive=nvme0,serial=deadbeef"
    args+=(--qemu-opts="$qemu")
    [[ -n "{{ cmd }}" ]] && args+=(--exec "{{ cmd }}")
    virtme-ng "${args[@]}"

# Test (verbose: 0=summary, 1=full — test-all uses summary mode)

test-all: (test-objectspace "0") (test-domain "0") (test-rarc "0") (test-nullcap "0") (test-domain-module "0") (test-panicked-thread "0") (test-nested-call "0") (test-revocable "0") test-pci-cap

test-domain verbose="1":
    @just _test domain_test {{ verbose }}

test-objectspace verbose="1":
    @just _test objectspace_test {{ verbose }}

test-rarc verbose="1":
    @just _test rarc_test {{ verbose }}

test-nullcap verbose="1":
    @just vng "modprobe ./rkernel/test/nullcap_test/nullcap_test.ko verbose={{ verbose }} && dmesg | grep -E 'nullcap_test:.*(PASS|FAIL|BENCH|tests passed)' && dmesg | grep -q 'leaked NullCap' && echo '[VERIFY] leaked NullCap warning detected' || echo '[VERIFY-FAIL] no leaked NullCap warning found'"

test-domain-module verbose="1":
    @just _test domain_module_test {{ verbose }}

test-panicked-thread verbose="1":
    @just _test panicked_thread_test {{ verbose }}

test-nested-call verbose="1":
    @just _test nested_call_test {{ verbose }}

test-revocable verbose="1":
    @just _test revocable_test {{ verbose }}

test-pci-cap:
    @just vng "echo 0000:00:0a.0 > /sys/bus/pci/drivers/rust_driver_auxiliary/unbind 2>/dev/null; modprobe ./rkernel/test/pci_cap_test/pci_cap_test.ko && echo 0000:00:0a.0 > /sys/bus/pci/drivers/pci_cap_test/bind 2>/dev/null; sleep 0.1; dmesg | grep -E 'pci_cap_test:.*(PASS|FAIL|BENCH|tests passed)'"

_test mod verbose:
    @just vng "modprobe ./rkernel/test/{{ mod }}/{{ mod }}.ko verbose={{ verbose }} && dmesg | grep -E '{{ mod }}:.*(PASS|FAIL|BENCH|tests passed)'"

# Lint & Format

fmt-check *args="":
    #!/usr/bin/env bash
    set -euo pipefail
    if [[ -n "{{ args }}" ]]; then
        files="{{ args }}"
    else
        files=$(find {{ rs_dirs }} -name '*.rs' -not -path '*/ext_crates/*')
    fi
    count=$(echo "$files" | wc -w)
    echo "Checking formatting for $count files..."
    echo "$files" | xargs rustup run nightly rustfmt --check --config-path {{ fmt_config }}
    echo "All files formatted correctly."

fmt *args="":
    #!/usr/bin/env bash
    set -euo pipefail
    if [[ -n "{{ args }}" ]]; then
        files="{{ args }}"
    else
        files=$(find {{ rs_dirs }} -name '*.rs' -not -path '*/ext_crates/*')
    fi
    count=$(echo "$files" | wc -w)
    echo "Formatting $count files..."
    echo "$files" | xargs rustup run nightly rustfmt --config-path {{ fmt_config }}
    echo "Done."

lint-super *args="":
    #!/usr/bin/env bash
    set -euo pipefail
    if [[ -n "{{ args }}" ]]; then
        targets="{{ args }}"
    else
        targets="{{ rs_dirs }}"
    fi
    matches=$(grep -rn 'use super::' --include='*.rs' --exclude-dir='ext_crates' $targets || true)
    if [[ -n "$matches" ]]; then
        echo "Error: Found 'use super::' — use crate:: paths instead."
        echo "$matches"
        exit 1
    fi
    echo "OK: No 'use super::' found."

lint-pub-fn:
    #!/usr/bin/env bash
    set -euo pipefail
    cap_dir="rust/kernel/rkernel/cap"
    violations=""
    while IFS= read -r file; do
        depth=0
        impl_depth=-1
        allowed=0
        lineno=0
        prev_line=""
        while IFS= read -r line; do
            lineno=$((lineno + 1))
            # Track impl block entry
            if [[ $depth -eq 0 ]] && [[ "$line" =~ ^[[:space:]]*impl[^{]*\{ ]]; then
                impl_depth=$depth
                allowed=0
                if [[ "$line" =~ Owned\< ]] || [[ "$line" =~ Revocable\< ]] || [[ "$line" =~ RevokeHandle\< ]] || [[ "$line" =~ "impl Monitor" ]]; then
                    allowed=1
                fi
            fi
            # Count braces
            opens="${line//[^\{]/}"
            closes="${line//[^\}]/}"
            depth=$(( depth + ${#opens} - ${#closes} ))
            # Check if we left the impl block
            if [[ $impl_depth -ge 0 ]] && [[ $depth -le $impl_depth ]]; then
                impl_depth=-1
                allowed=0
            fi
            # Detect pub fn (skip pub(crate), pub(in, pub(super), and lint:pub-fn-ok on prev line)
            if [[ "$line" =~ ^[[:space:]]*(pub[[:space:]]+unsafe[[:space:]]+fn|pub[[:space:]]+fn)[[:space:]] ]]; then
                if [[ "$line" =~ pub\(crate\) ]] || [[ "$line" =~ pub\(in ]] || [[ "$line" =~ pub\(super\) ]]; then
                    prev_line="$line"
                    continue
                fi
                if [[ "$prev_line" =~ "lint:pub-fn-ok" ]]; then
                    prev_line="$line"
                    continue
                fi
                if [[ $allowed -ne 1 ]]; then
                    violations+="  $file:$lineno: $line"$'\n'
                fi
            fi
            prev_line="$line"
        done < "$file"
    done < <(find "$cap_dir" -name '*.rs' | sort)
    if [[ -n "$violations" ]]; then
        echo "Error: pub fn outside Monitor/Owned/Revocable in cap/:"
        echo "$violations"
        exit 1
    fi
    echo "OK: All pub fn in cap/ are inside Monitor/Owned/Revocable."

# Utility

build-baseline dir="../build_baseline": _fmt _lint
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir -p {{ dir }}
    grep -v '^CONFIG_RKERNEL=' rkernel/LINUX_CONFIG > {{ dir }}/.config
    make O={{ dir }} LLVM=1 olddefconfig
    make O={{ dir }} LLVM=1 CLIPPY=1 -j$(nproc)

clean:
    make O={{ build_dir }} clean

rust-analyzer: _config
    make {{ flags }} rust-analyzer

# Private

_fmt:
    @just fmt-check

_lint:
    @just lint-super
    @just lint-pub-fn

_config: _build_dir
    cp rkernel/LINUX_CONFIG {{ build_dir }}/.config
    make O={{ build_dir }} LLVM=1 olddefconfig

_build_dir:
    mkdir -p {{ build_dir }}

# Benchmark NVMe read
# Usage: just bench <driver> [runs]
#   nvme: C driver, rnvme: official Rust, rnvme-domain: isolated rnvme

bench driver="nvme" runs="5":
    #!/usr/bin/env bash
    set -euo pipefail
    case "{{ driver }}" in
        nvme)
            just vng "./rkernel/scripts/bench-nvme.sh {{ runs }}"
            ;;
        rnvme)
            just vng "./rkernel/scripts/bench-nvme.sh {{ runs }} rnvme /dev/nvme0n1"
            ;;
        rnvme-domain)
            just vng "./rkernel/scripts/bench-nvme.sh {{ runs }} ./rkernel/rm-drivers/rnvme_domain/rnvme_domain.ko /dev/nvme0n1"
            ;;
        *)
            echo "Unknown driver: {{ driver }}"
            echo "Available: nvme, rnvme, rnvme-domain"
            exit 1
            ;;
    esac

bench-all runs="5":
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir -p rkernel/.tmp/bench
    for drv in nvme rnvme rnvme-domain; do
        echo "=== Benchmarking $drv ===" >&2
        just bench "$drv" {{ runs }} > "rkernel/.tmp/bench/${drv}.csv"
    done
    echo "All benchmarks saved to rkernel/.tmp/bench/" >&2

bench-fio driver="nvme" runs="40" bs="":
    #!/usr/bin/env bash
    set -euo pipefail
    VFIO_PCI="0000:00:0b.0"
    TS="${BENCH_FIO_TS:-$(date +%Y%m%d-%H%M%S)}"
    OUTDIR="rkernel/.tmp/bench-fio/$TS"
    BS_ENV="{{ if bs != "" { "export BENCH_BS='" + bs + "';" } else { "" } }}"
    mkdir -p "$OUTDIR"
    case "{{ driver }}" in
        nvme)
            just vng "${BS_ENV} ./rkernel/scripts/bench-fio-randread.sh /dev/nvme0n1 {{ runs }}" > "$OUTDIR/nvme.csv"
            echo "Saved: $OUTDIR/nvme.csv" >&2
            ;;
        rnvme)
            just vng "${BS_ENV} echo $VFIO_PCI > /sys/bus/pci/drivers/nvme/unbind 2>/dev/null; sleep 0.3; echo $VFIO_PCI > /sys/bus/pci/drivers/rnvme/bind; sleep 2; ./rkernel/scripts/bench-fio-randread.sh /dev/nvme0n1 {{ runs }}" > "$OUTDIR/rnvme.csv"
            echo "Saved: $OUTDIR/rnvme.csv" >&2
            ;;
        rnvme-domain)
            just vng "${BS_ENV} echo $VFIO_PCI > /sys/bus/pci/drivers/nvme/unbind 2>/dev/null; sleep 0.3; modprobe ./rkernel/rm-drivers/rnvme_domain/rnvme_domain.ko; sleep 2; ./rkernel/scripts/bench-fio-randread.sh /dev/nvme0n1 {{ runs }}" > "$OUTDIR/rnvme-domain.csv"
            echo "Saved: $OUTDIR/rnvme-domain.csv" >&2
            ;;
        all)
            export BENCH_FIO_TS="$TS"
            echo "=== nvme (C) ===" >&2
            just bench-fio nvme {{ runs }} '{{ bs }}'
            echo "" >&2
            echo "=== rnvme (Rust) ===" >&2
            just bench-fio rnvme {{ runs }} '{{ bs }}'
            echo "" >&2
            echo "=== rnvme_domain (isolated) ===" >&2
            just bench-fio rnvme-domain {{ runs }} '{{ bs }}'
            echo "" >&2
            echo "All results saved to $OUTDIR/" >&2
            ;;
        *)
            echo "Unknown driver: {{ driver }}" >&2
            echo "Available: nvme, rnvme, rnvme-domain, all" >&2
            exit 1
            ;;
    esac

bench-plot:
    uv run rkernel/scripts/bench-plot.py rkernel/.tmp/bench/

# PostgreSQL pgbench on NVMe
# Usage: just bench-pgbench <driver> [scale] [clients] [duration]

# nvme: C driver, rnvme: official Rust, rnvme-domain: isolated rnvme
bench-pgbench driver="nvme" scale="10" clients="4" duration="60":
    #!/usr/bin/env bash
    set -euo pipefail
    case "{{ driver }}" in
        nvme)
            just vng "./rkernel/scripts/bench-pgbench.sh {{ scale }} {{ clients }} {{ duration }}"
            ;;
        rnvme)
            just vng "./rkernel/scripts/bench-pgbench.sh {{ scale }} {{ clients }} {{ duration }} rnvme /dev/nvme0n1"
            ;;
        rnvme-domain)
            just vng "./rkernel/scripts/bench-pgbench.sh {{ scale }} {{ clients }} {{ duration }} ./rkernel/rm-drivers/rnvme_domain/rnvme_domain.ko /dev/nvme0n1"
            ;;
        *)
            echo "Unknown driver: {{ driver }}"
            echo "Available: nvme, rnvme, rnvme-domain"
            exit 1
            ;;
    esac

# HammerDB TPC-C on NVMe
# Usage: just bench-hammerdb <driver> [warehouses] [vu] [duration]
#   nvme: C driver, rnvme: official Rust, rnvme-domain: isolated rnvme
#   warehouses: TPC-C warehouse count (default: 50, ~5GB data > 64MB shared_buffers)
#   vu: virtual users (default: 8)

# duration: test duration in minutes (default: 5)
bench-hammerdb driver="nvme" warehouses="50" vu="8" duration="5":
    #!/usr/bin/env bash
    set -euo pipefail
    case "{{ driver }}" in
        nvme)
            just vng "./rkernel/scripts/bench-hammerdb.sh {{ warehouses }} {{ vu }} {{ duration }}"
            ;;
        rnvme)
            just vng "./rkernel/scripts/bench-hammerdb.sh {{ warehouses }} {{ vu }} {{ duration }} rnvme /dev/nvme0n1"
            ;;
        rnvme-domain)
            just vng "./rkernel/scripts/bench-hammerdb.sh {{ warehouses }} {{ vu }} {{ duration }} ./rkernel/rm-drivers/rnvme_domain/rnvme_domain.ko /dev/nvme0n1"
            ;;
        *)
            echo "Unknown driver: {{ driver }}"
            echo "Available: nvme, rnvme, rnvme-domain"
            exit 1
            ;;
    esac
