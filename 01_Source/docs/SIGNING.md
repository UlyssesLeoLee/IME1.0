# Code signing of the installer (IME1.0)

> Status: **the installer is not signed.** `scripts\sign-installer.ps1` exists and is ready to use, but
> it has **never been run with a real certificate**: this machine has no code signing certificate and no
> Windows SDK (there is no `signtool.exe` here), so signing, the RFC 3161 timestamp and the `/pa`
> verification were **not** executed. What *was* executed on this machine is listed in section 10. Read
> that before you trust the script with a certificate you care about.
>
> The installer is a pre-release in every other sense too: the text service DLL it carries has never been
> verified in a virtual machine (see `TSF_VM_TEST_PLAN.md`), and the installer itself has never run on a
> real Windows system.

## 1. Why signing matters for this installer

`scripts\build-installer.js` writes `dist\ProgrammingIME-<version>-win-x64-setup.exe` and stops there.
The file is not code signed, so:

- **Windows SmartScreen** shows "Unknown publisher" (the publisher name comes from the signature; without
  one there is none), and the "more info" link is the only way past the warning. Users have to click
  through it on every machine that never saw the file before.
- **Enterprise policy** can go further than a warning: AppLocker, WDAC, Smart App Control and many
  antivirus products refuse unsigned or untrusted executables outright. The installer also **elevates
  itself** (it restarts itself with `runas`, see `INSTALLER.md` section 4), which makes an unsigned
  self-elevating binary the worst case for those policies - and the "unknown publisher" is shown in the
  UAC prompt as well.
- **The `.sha256` sidecar does not replace a signature.** It proves that the file was not damaged or
  truncated on the way from the build machine to the user's machine. It does **not** prove who produced
  the file: whoever can change the installer can also recompute the hash and rewrite the sidecar. The
  sidecar is written next to the file it describes, so it travels in the same trust channel as the file
  itself. Origin needs a signature; integrity only needs a hash.

Signing also removes one concrete weakness that `INSTALLER.md` section 4 names: an unsigned installer
that re-runs `current_exe()` from a user writable folder (the Downloads folder) can be swapped by another
program of the same user between the UAC prompt and the elevated run. A signature does not close that
window completely (the file must still be replaced by a signed one), but it removes the "replace it with
anything" part, because the loader will refuse an unsigned replacement.

## 2. What the script does

`scripts\sign-installer.ps1` (Windows PowerShell 5.1, no modules, no dependencies beyond `signtool.exe`):

1. picks the installer to sign: the given `-Installer`, or the newest `dist\ProgrammingIME-*-win-x64-setup.exe`;
2. finds `signtool.exe` (see section 5) - nothing is guessed, if it cannot be found the script lists every
   place it looked;
3. reads the certificate and the password from the drop folder `scripts\certs\`;
4. signs the file **in place** with SHA-256 (`/fd sha256`) and an RFC 3161 timestamp
   (`/tr http://timestamp.digicert.com /td sha256`);
5. **verifies** the result with `signtool verify /pa /v` and stops with a loud error if that fails;
6. **re-runs the installer's own `--dry-run`** on the signed file and checks that the plan is printed,
   that the payload was verified, and that nothing was created;
7. **rewrites the `.sha256` sidecar** (the digest of the file changes when it is signed) and prints the
   new digest.

Step 6 is the point of the script. The payload lives in a PE overlay and the Authenticode certificate
table is appended **after** that overlay (section 7). A signed installer whose payload can no longer be
located looks perfectly healthy to a user - the signature is valid, Windows runs it - and is broken on
every machine. The only way to know is to run the signed file. The dry run is the same check
`build-installer.js` does on the unsigned file (it proves the exe finds and verifies its own payload),
repeated after signing.

`-VerifyOnly` runs steps 2, 5 and 6 on an existing file and signs nothing.

## 3. The certificate: where it goes

**The convention (one, fixed):**

```
01_Source\scripts\certs\cert.pfx            the code signing certificate (PKCS#12)
01_Source\scripts\certs\cert.password.txt   its password, first non-empty line
```

The script looks for exactly these two names. Other names work with `-CertFile` / `-PasswordFile`, and the
password can be given directly with `-Password` (it overrides the password file).

The folder is a **drop folder on the machine that signs**: it is not a certificate store and it is not
part of the repository. See `scripts\certs\README.md` for the `.gitignore` lines that keep it out of git.

Two things to know before you put a certificate there:

- A committed `.pfx` is a committed **private key**. It has to be revoked, not just deleted from the
  history. The `.gitignore` lines exist to prevent that, not to clean up after it.
- `signtool.exe` takes the password as `/p <password>`, so the password is on its command line and is
  briefly visible to other processes of the same user. That is a limitation of `signtool`, not of this
  script; the password file only keeps it out of your shell history.

A code signing certificate from a public CA is the normal answer (an OV or EV certificate; EV additionally
gives the organization name in the UAC prompt without the reputation requirement). A self signed
certificate is fine for a test, but see section 6.

## 4. Running it

From the repository root:

```powershell
# 1. put the certificate and its password into the drop folder
#    01_Source\scripts\certs\cert.pfx
#    01_Source\scripts\certs\cert.password.txt

# 2. build the installer (unsigned) if you do not have one yet
cd 01_Source
node scripts\build-installer.js
cd ..

# 3. sign it
powershell.exe -NoProfile -ExecutionPolicy Bypass -File 01_Source\scripts\sign-installer.ps1
```

Without `-Installer` the script signs the newest `01_Source\dist\ProgrammingIME-*-win-x64-setup.exe`.
The full command line:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File 01_Source\scripts\sign-installer.ps1 `
    [-Installer <path to the .exe>] `
    [-CertFolder <folder with the .pfx>] `
    [-CertFile <name of the .pfx>] `
    [-PasswordFile <file with the password>] `
    [-Password <the password>] `
    [-SignTool <full path to signtool.exe>] `
    [-TimestampUrl <RFC 3161 server>] `
    [-VerifyOnly] `
    [-Help]
```

Useful variants:

```powershell
# only check an existing signature and re-run the dry run (signs nothing)
powershell.exe -NoProfile -ExecutionPolicy Bypass -File 01_Source\scripts\sign-installer.ps1 -VerifyOnly

# sign a specific file with a specific signtool.exe
powershell.exe -NoProfile -ExecutionPolicy Bypass -File 01_Source\scripts\sign-installer.ps1 `
    -Installer "01_Source\dist\ProgrammingIME-0.2.1-win-x64-setup.exe" `
    -SignTool "C:\Program Files (x86)\Windows Kits\10\bin\10.0.22621.0\x64\signtool.exe"

# sign without a timestamp (the signature then dies with the certificate; for offline tests)
powershell.exe -NoProfile -ExecutionPolicy Bypass -File 01_Source\scripts\sign-installer.ps1 -TimestampUrl ""
```

The script needs to be online for the timestamp server only. The verification step may additionally need
network access to the CRL/OCSP endpoints of the certificate authority, because `/pa` checks the
revocation status; on a machine without those endpoints the verification can fail with a revocation
error even though the signature itself is good.

## 5. Exit codes

| Code | Meaning | What to do |
|---|---|---|
| 0 | success: signed, verified, dry run passed, sidecar rewritten | distribute the file and the new `.sha256` |
| 2 | usage or configuration error (bad parameter, missing password file, no installer in `dist\`) | fix the command line or build the installer first |
| 3 | **no certificate found** in the drop folder | put `cert.pfx` into `01_Source\scripts\certs\` (or pass `-CertFile` / `-CertFolder`) |
| 4 | **the certificate could not be opened** (wrong password) | check `cert.password.txt` / `-Password` |
| 5 | **signtool.exe not found** | install the Windows SDK, or pass `-SignTool "<full path>"`; the error lists every place that was searched |
| 6 | **signing failed** | read the signtool output in the error; the file was changed by the failed attempt, rebuild it |
| 7 | **the signature did not verify** | for a self signed certificate: trust its root in "Trusted Root Certification Authorities" of the local machine; for a real certificate: read the signtool output |
| 8 | **the dry run after signing failed** | the signed file does not work; do not distribute it, rebuild it and report the output |
| 9 | **the installer carries no payload** | the loader did not find the overlay in the signed file; do not distribute it (this is exactly the failure the dry run exists to catch) |

Every failure prints the reason and the relevant output of the tool that failed. A non-zero exit code
always means: **the file in `dist\` must not be distributed.** A failed run may have left the file signed
but unverified; the sidecar is written only after every check passed, so a stale sidecar is a visible
sign that the run did not finish.

## 6. What the script proves - and what it does not

**It proves**, for the file it just signed:

- the Authenticode signature is present and valid according to `signtool verify /pa /v` (that means: the
  hash of the file matches the signature, the signature is intact, the certificate chains to a root the
  machine trusts, and the certificate was not revoked at the time of the check);
- the signature carries a SHA-256 file digest and an RFC 3161 timestamp (so the signature stays valid
  after the certificate expires);
- the signed file still finds and verifies its own payload (`--dry-run` prints the plan, verifies every
  file in the overlay and creates nothing);
- the `.sha256` sidecar matches the signed file.

**It does not prove**:

- **that the payload is safe.** The signature covers the whole file, so the signature proves that *the
  publisher* produced this exact byte sequence. It says nothing about the content of the payload: a
  signed installer can carry a malicious `pime_tsf.dll`. The signature is a statement about the origin of
  the file, not about the quality of what is inside it. The DLL inside the payload has still never been
  verified in a virtual machine (`TSF_VM_TEST_PLAN.md`), and signing does not change that.
- **that the installer works.** The dry run only proves that the payload is found and verified. The real
  host test - register the text service, type in a host application, install, upgrade, uninstall - is
  still missing and is described in `TSF_VM_TEST_PLAN.md` and `INSTALLER.md` section 14.1. A signed
  installer is not a tested installer.
- **that the chain is trusted on the user's machine.** `/pa` uses the default authentication policies of
  the machine that runs the check. On a machine without the root certificate in its store the same file
  fails verification. For a self signed test certificate, add its root to "Trusted Root Certification
  Authorities" of the **local machine** (or the current user) first, otherwise `/pa` rejects it with a
  chain error even though the signature itself is correct.
- **that the timestamp authority was reachable**. A signing run whose timestamp request failed does not
  produce a usable long term signature; `signtool` reports that as a signing error (exit code 6 here).

## 7. The known caveats of signing this file

The installer is not a normal PE file, and that is the whole reason this script exists.

```
file := PE stub | payload blob | footer | [ zero padding | Authenticode certificate table ]
                        ^^^^^^^^^^^^^^^^^^^^^^^^^^^^ the overlay
```

- The **payload is a PE overlay**: the files to install (the text service DLL, the core, the license, the
  `.vsix`, `meta.json`) are appended to the finished stub. Windows ignores the bytes after the last
  section, so this works, but the loader has to find the overlay again.
- The loader locates the overlay through the **PE security directory**: the certificate table is appended
  **after** the overlay and the security directory (data directory 4) says where it starts. The end of
  the overlay is therefore "the start of the certificate table", not "the end of the file".
- `signtool` pads the data in front of the certificate table to an **8 byte boundary**, so up to **7
  bytes** of zero padding can sit between the end of the footer and the start of the table. The loader
  tolerates exactly that (`crates\pime-setup\src\payload.rs`, `find_footer`).
- This is **unit tested but was never checked against a real signature**: the test
  `st_035_payload_survives_a_signature_table_with_alignment_padding` builds a synthetic PE with a
  certificate table and 5 bytes of padding. No real `signtool` output ever went through the loader.
  `INSTALLER.md` section 1 and section 8 say the same.
- That is exactly why the script **re-runs `--dry-run` after signing** instead of trusting the unit test.
  The dry run is the only thing that exercises the real layout with a real file.

Consequences to keep in mind:

- The unsigned installer and the signed installer are **different files** (the certificate table is added,
  so the size and the SHA-256 change). The `.sha256` sidecar of the unsigned build is **wrong** for the
  signed file; the script rewrites it.
- The **build output changes**: `build-installer.js` prints the digest of the unsigned file. After signing
  the digest is a different one, and the signed file is the one that has to be distributed.
- Signing **after** the payload was appended is the only supported order. Signing the stub first and
  appending the payload afterwards would put the overlay *after* the certificate table, which the loader
  does not support.
- A **self signed** certificate removes the SmartScreen warning only on machines that trust it. It is a
  test tool, not a distribution method.

## 8. The `.gitignore` line that has to be added

`01_Source/.gitignore` already ignores `*.pfx`, but **not** `cert.password.txt` and not any other file
that may end up in the drop folder. Add these three lines to `01_Source/.gitignore` (the file in the
repository root keeps its paths relative to the root, this one keeps them relative to `01_Source`):

```
# code signing material: never commit the certificate or its password (see 01_Source/docs/SIGNING.md)
scripts/certs/*
!scripts/certs/.gitkeep
!scripts/certs/README.md
```

Then check that nothing of the secret material is tracked:

```
git ls-files 01_Source/scripts/certs
```

It must print only `.gitkeep` and `README.md`.

## 9. The `.sha256` sidecar has to be regenerated after signing

`build-installer.js` writes `dist\ProgrammingIME-<version>-win-x64-setup.exe.sha256` next to the installer,
in the GNU format (`<digest> *<file name>`), for the **unsigned** file. Signing appends the certificate
table, so the digest changes and the old sidecar no longer matches the file it describes.

The script rewrites the sidecar after signing and prints the new digest. The sidecar is written **only
after** the signature verified and the dry run passed, so a sidecar that matches the file always
describes a file that passed every check. If the script failed, the sidecar is stale on purpose: it does
not match the file, and a mismatch is a visible sign that something went wrong.

Check the sidecar by hand at any time:

```powershell
Get-FileHash .\dist\ProgrammingIME-0.2.1-win-x64-setup.exe -Algorithm SHA256
Get-Content    .\dist\ProgrammingIME-0.2.1-win-x64-setup.exe.sha256
```

## 10. What was verified on the development machine, and what was not

Verified here (Windows PowerShell 5.1.26100.9818):

- `-Help` prints the usage and exits 0.
- The script parses without errors (`PSParser::Tokenize` and `Parser::ParseFile`).
- The script contains **no non-ASCII byte** (Windows PowerShell 5.1 reads a UTF-8 file without a BOM as
  ANSI and would break on a non-ASCII literal).
- The whole flow (sign, verify, dry run, sidecar) was run end to end against a **fake `signtool`** that
  was written for the test: it appends a 128 byte "certificate table" after the overlay and updates the PE
  security directory the way the real `signtool` does (8 byte aligned). With that file the **real**
  `--dry-run` of the real installer still found and verified its payload after the table was appended.
  That is evidence for the layout assumption of section 7 - it is **not** an Authenticode signature, no
  certificate and no timestamp were involved.
- The same fake with a deliberately broken layout (table appended, security directory **not** updated) made
  the script stop with exit code 9 ("the installer carries no payload") and with **no** sidecar written.
  The failure mode the script exists for is caught.
- The wrong password branch (exit code 4), the missing certificate branch (exit code 3) and the missing
  `signtool.exe` branch (exit code 5, listing every searched place) were run and produce their messages.

**Not** verified here, because this machine has neither a certificate nor the Windows SDK:

- the real `signtool sign` with a real `.pfx`, the RFC 3161 timestamp against
  `http://timestamp.digicert.com`, and the real `signtool verify /pa /v` (including the behaviour of
  `/pa` with a self signed certificate, which is why section 6 tells you to trust the test root first);
- the behaviour of a real `signtool` certificate table against the loader - the fake reproduces the
  *layout* (offset, size, 8 byte padding) that the loader reads, not the PKCS#7 structure that the real
  tool writes;
- the SmartScreen reaction, the UAC prompt text and the enterprise policy behaviour;
- the whole installation on a real system (still missing, see `TSF_VM_TEST_PLAN.md`).

The first real run should be done on a throwaway machine with a **test certificate**, and its output should
be compared with the expectations of sections 2 and 6 before a real certificate is used for a release.
