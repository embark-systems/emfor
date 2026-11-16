#!/bin/sh
# Sign the deb and rpm packages of packages/dist/ and check the signatures the
# way a user would, with only the public key.
#
#   SIGN_KEY=<fingerprint> tools/sign-packages.sh sign   [DIST_DIR]
#   tools/sign-packages.sh verify [DIST_DIR]
#
# DIST_DIR defaults to packages/dist. Nothing here is run by the build or the
# CI; it is the release step described in packages/README.md ("Signing").
#
# sign    Needs gpg, sha256sum (or shasum) and, for an rpm, rpmsign, which is
#         a Linux tool: run it on a Linux host or in a container that has the
#         secret key (a hardware key is not reachable from a container unless
#         its gpg-agent socket is mounted). It
#           - signs every rpm in place (rpmsign --addsign);
#           - writes a detached signature NAME.deb.asc for every deb (apt
#             verifies a repository, not a single file, so a deb has no
#             embedded signature that a tool would check);
#           - writes SHA256SUMS of all packages, and SHA256SUMS.asc;
#           - exports the public key as emfor-release.asc (armored, for
#             `rpm --import` and `gpg --import`) and emfor-release.gpg
#             (binary, for `gpgv --keyring`).
#         The checksums are made after rpmsign, which changes the rpm.
# verify  Needs docker. In a clean Debian and a clean Rocky Linux container
#         that hold only DIST_DIR (read-only) and the public key from it, it
#         checks SHA256SUMS and every detached signature with gpgv, and every
#         rpm with rpm -K; then it changes one byte of a copy of each package
#         and requires the check to fail, and requires rpm -K to report a
#         missing key before the key is imported, so that a pass means
#         something. Images: DEBIAN_IMAGE, ROCKY_IMAGE.
set -eu

cmd=${1:-}
cd "$(dirname "$0")/.."
dist=${2:-packages/dist}
debian_image=${DEBIAN_IMAGE:-debian:bookworm}
rocky_image=${ROCKY_IMAGE:-rockylinux:9}

die() { echo "sign-packages: $*" >&2; exit 1; }

sums() { # SHA256SUMS of the packages in the current directory
    if command -v sha256sum >/dev/null 2>&1; then sha256sum "$@"; else shasum -a 256 "$@"; fi
}

packages_in() { # a list of the *.deb and *.rpm in $dist, or an error
    [ -d "$dist" ] || die "$dist is not a directory"
    n=0
    for f in "$dist"/*.deb "$dist"/*.rpm; do
        [ -e "$f" ] && n=$((n + 1))
    done
    [ "$n" -gt 0 ] || die "no deb or rpm in $dist"
}

sign() {
    [ -n "${SIGN_KEY:-}" ] || die "set SIGN_KEY to the fingerprint of the signing key"
    command -v gpg >/dev/null 2>&1 || die "gpg is not installed"
    gpg --list-secret-keys "$SIGN_KEY" >/dev/null 2>&1 || die "no secret key $SIGN_KEY in this gpg"
    packages_in
    for f in "$dist"/*.rpm; do
        [ -e "$f" ] || continue
        command -v rpmsign >/dev/null 2>&1 \
            || die "$f needs rpmsign (a Linux tool): sign on a Linux host or in a container"
        rpmsign --addsign --define "_gpg_name $SIGN_KEY" "$f"
    done
    for f in "$dist"/*.deb; do
        [ -e "$f" ] || continue
        gpg --batch --yes --local-user "$SIGN_KEY" --armor --detach-sign --output "$f.asc" "$f"
    done
    (
        cd "$dist"
        rm -f SHA256SUMS SHA256SUMS.asc
        sums $(ls -1 *.deb *.rpm 2>/dev/null) > SHA256SUMS
        gpg --batch --yes --local-user "$SIGN_KEY" --armor --detach-sign --output SHA256SUMS.asc SHA256SUMS
        gpg --batch --yes --armor --export "$SIGN_KEY" > emfor-release.asc
        gpg --batch --yes --export "$SIGN_KEY" > emfor-release.gpg
    )
    echo "signed with $(gpg --list-secret-keys --with-colons "$SIGN_KEY" | awk -F: '/^fpr/{print $10; exit}')"
    ls -l "$dist"
}

verify() {
    command -v docker >/dev/null 2>&1 || die "docker is not installed"
    for f in SHA256SUMS SHA256SUMS.asc emfor-release.asc emfor-release.gpg; do
        [ -f "$dist/$f" ] || die "$dist/$f is missing (run sign first)"
    done
    packages_in
    abs=$(cd "$dist" && pwd)

    echo "== Debian: gpgv and sha256sum, the public key only"
    docker run --rm -v "$abs":/d:ro "$debian_image" sh -c '
        set -eu
        k=/d/emfor-release.gpg
        gpgv --keyring "$k" /d/SHA256SUMS.asc /d/SHA256SUMS
        (cd /d && sha256sum -c SHA256SUMS)
        for f in /d/*.deb; do
            [ -e "$f" ] || continue
            gpgv --keyring "$k" "$f.asc" "$f"
        done
        # a changed byte must fail, in the package and in the sums
        mkdir /t; cp /d/SHA256SUMS /d/SHA256SUMS.asc /t/
        for f in /d/*.deb /d/*.rpm; do
            [ -e "$f" ] || continue
            cp "$f" /t/; [ -e "$f.asc" ] && cp "$f.asc" /t/
        done
        for f in /t/*.deb; do
            [ -e "$f" ] || continue
            printf x >> "$f"
            if gpgv --keyring "$k" "$f.asc" "$f" 2>/dev/null; then echo "FAIL: a changed deb verified"; exit 1; fi
            echo "ok: a changed $(basename "$f") does not verify"
        done
        printf x >> /t/SHA256SUMS
        if gpgv --keyring "$k" /t/SHA256SUMS.asc /t/SHA256SUMS 2>/dev/null; then echo "FAIL: changed SHA256SUMS verified"; exit 1; fi
        echo "ok: a changed SHA256SUMS does not verify"
        echo "ok: Debian"
    '

    has_rpm=no
    for f in "$dist"/*.rpm; do [ -e "$f" ] && has_rpm=yes; done
    if [ "$has_rpm" = yes ]; then
        echo "== Rocky Linux: rpm -K"
        docker run --rm -v "$abs":/d:ro "$rocky_image" sh -c '
            set -eu
            mkdir /t
            for f in /d/*.rpm; do
                # before the key is imported the signature must be reported as not checkable
                out=$(rpm -K "$f" 2>&1 || true)
                case "$out" in *"signatures OK"*) echo "FAIL: $f is OK without the key: $out"; exit 1 ;; esac
                echo "ok: without the key: $out"
            done
            rpm --import /d/emfor-release.asc
            for f in /d/*.rpm; do
                out=$(rpm -K "$f" 2>&1) || { echo "FAIL: $out"; exit 1; }
                case "$out" in *"signatures OK"*) echo "ok: $out" ;; *) echo "FAIL: $out"; exit 1 ;; esac
                cp "$f" /t/
                g=/t/$(basename "$f")
                printf x >> "$g"
                if rpm -K "$g" >/dev/null 2>&1; then echo "FAIL: a changed rpm verified"; exit 1; fi
                echo "ok: a changed $(basename "$f") does not verify"
            done
            echo "ok: Rocky Linux"
        '
    fi
    echo "verified: $dist"
}

case "$cmd" in
    sign) sign ;;
    verify) verify ;;
    *) sed -n '2,10p' "$0" | sed 's/^# \{0,1\}//' >&2; exit 2 ;;
esac
