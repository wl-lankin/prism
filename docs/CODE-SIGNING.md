# Code signing (Windows)

The app, the FFmpeg tools (`ffmpeg.exe`, `ffprobe.exe`) and the installer are signed with [Azure Artifact Signing](https://learn.microsoft.com/azure/artifact-signing/). The certificate names "Wolfgang Linz" as the publisher; Microsoft has verified the identity. macOS builds are not signed.

## Azure

- Account `wolfganglinz-signing` (resource group `code-signing`, North Europe, endpoint `https://neu.codesigning.azure.net`), certificate profile `wolfganglinz-public` (Public Trust). The certificates live only a few days and Azure renews them; the timestamp keeps a signature valid afterwards.
- App registration `github-code-signing` with the role "Artifact Signing Certificate Profile Signer" on this account only. It has no password: a federated credential trusts GitHub OIDC tokens for `repo:wl-lankin@27097016/prism@1428752064:environment:release`.

## GitHub

- Environment `release`: the Windows leg of the release workflow and the check workflow run in it.
- Repository variables `AZURE_CLIENT_ID`, `AZURE_TENANT_ID`, `AZURE_SUBSCRIPTION_ID` (IDs, not secrets).
- `.github/actions/windows-signing` downloads Microsoft's signing client (version and SHA-256 pinned), writes its `metadata.json` and a Tauri config with `bundle.windows.signCommand`. `sign.ps1` fetches a fresh OIDC token for each file and calls `signtool` with the client DLL.
- Tauri signs the app, the `ffmpeg`/`ffprobe` sidecars (`externalBin`) and the installer during `tauri build`, so the updater `.sig` files are made over the signed installer. Prism has no bundle resources that need signing by hand.
- `windows-signing-check.yml` builds and signs without a release (no updater key) and checks the signatures inside the installer. It runs on pull requests that change the signing setup, or by hand.
