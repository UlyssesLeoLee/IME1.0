# Certificate drop folder

This folder is the place where the code signing certificate lives **on the machine that signs the
installer**. It is not part of the repository: the certificate and its password must never be
committed.

## What goes here

| File | Meaning |
|---|---|
| `cert.pfx` | the code signing certificate (PKCS#12, the export of the private key with the code signing usage) |
| `cert.password.txt` | the password of the `.pfx`, first non-empty line, surrounding whitespace removed |
| `README.md` | this file (tracked) |
| `.gitkeep` | keeps the empty folder in the repository (tracked) |

`scripts\sign-installer.ps1` looks for exactly these two names (`cert.pfx`, `cert.password.txt`).
Other names work too, but then they have to be passed on the command line:

```
powershell.exe -NoProfile -ExecutionPolicy Bypass -File 01_Source\scripts\sign-installer.ps1 `
    -CertFile my-company.pfx -PasswordFile my-company.password.txt
```

The password can also be given directly with `-Password <text>` (it overrides the password file).

## The .gitignore lines that have to be added

`01_Source/.gitignore` already ignores `*.pfx`, but **not** the password file and not any other file
that may end up here. Add these three lines to `01_Source/.gitignore` (not to the `.gitignore` in the
repository root, so that the paths stay relative to `01_Source`):

```
# code signing material: never commit the certificate or its password (see 01_Source/docs/SIGNING.md)
scripts/certs/*
!scripts/certs/.gitkeep
!scripts/certs/README.md
```

Check afterwards that nothing of the secret material is tracked:

```
git ls-files 01_Source/scripts/certs
```

It must print only `.gitkeep` and `README.md`.

## Rules

- **Never commit** `cert.pfx`, `cert.password.txt` or anything else that is not in the table above.
  A committed `.pfx` is a committed private key: it has to be revoked, not just deleted from the
  history.
- Delete the two secret files from this folder when the signing run is done. The folder is only a
  drop folder, not a certificate store.
- The password is still handed to `signtool.exe` on its command line, so it is briefly visible to
  other processes of the same user while signing. That is a limitation of `signtool`, not of this
  folder (see `01_Source/docs/SIGNING.md`).
- The signed installer and its `.sha256` sidecar are **not** secrets and go to `01_Source/dist`.

See `01_Source/docs/SIGNING.md` for the full procedure and for what signing does and does not prove.
