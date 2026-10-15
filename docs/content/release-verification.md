---
title: "Release Verification"
weight: 160
---

Every release ships a `SHA256SUMS` file and its minisign signature,
`SHA256SUMS.minisig`. The public key lives in the repository at
`dist/minisign.pub`.

## Verify a download

Install `minisign` (for example `apt install minisign` or
`brew install minisign`), download the release archive together with
`SHA256SUMS` and `SHA256SUMS.minisig` from the release page, and save the
public key next to them:

```sh
$ curl -LO https://raw.githubusercontent.com/KilimcininKorOglu/r3v3rs3/main/dist/minisign.pub
```

Then verify the checksums:

```sh
$ minisign -Vm SHA256SUMS -p minisign.pub
Signature and comment signature verified
```

`minisign` exits with a non-zero status when the signature does not match.
When the signature is good, check the archive against the checksums:

```sh
$ sha256sum --check --ignore-missing SHA256SUMS
r3v3rs3-x86_64-unknown-linux-gnu.tar.xz: OK
```

Docker images carry no separate signature. Their digest, which
`docker pull` prints, identifies the image bytes that the registry serves.
