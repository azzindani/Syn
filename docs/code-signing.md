# Code signing

Syn's Windows installer is **not code-signed yet**. Windows SmartScreen
treats any program it has no history of, and no signature on, as unknown, and
shows "Windows protected your PC" with "Unknown publisher". That is what a
person sees when they run `Syn-<version>-setup.exe` from a download. It is not
a fault in the installer, and nothing in the code can switch it off: it needs
a signing certificate, which is issued to an identity.

Until then, see [Troubleshooting](troubleshooting.md) for how to get past the
prompt (More info, then Run anyway, or Unblock the file first).

## What is wired

`.github/workflows/ci.yml`, in the `package` job, signs the installer through
[SignPath](https://about.signpath.io/product/open-source) when the project is
enrolled, and is otherwise inert:

- **Only on a `v*` tag.** A signing request is approved by a person; a push
  to `main` does not need one.
- **Only the installer.** It is what a download runs first. The zip's
  contents (`syn.exe`, `ui.exe` and the rest) are not signed yet; files taken
  from a downloaded zip are still marked as downloaded, so SmartScreen can ask
  about them too. Unblock the zip before extracting it. Signing them means
  signing before the zip and the installer are made (`scripts/package.ps1`),
  and the same `<pe-file>` rule in the artifact configuration below.
- **Skipped, not failed, without the secret.** With `SIGNPATH_API_TOKEN`
  missing, a tagged release still goes out, unsigned, with a warning on the run.
- **Checked after.** The step that puts the signed file where the release takes
  it from refuses to go on unless `Get-AuthenticodeSignature` says `Valid`.

The step has not run against a real SignPath account yet, so expect to adjust
it when the first tagged release is signed.

## Turning it on

SignPath Foundation signs open-source projects for free. It asks for an
OSI-approved licence (Syn is MIT), a public repository, free downloads, active
maintenance, a build that runs from the repository in GitHub Actions, and a
published code signing policy (the section in the [README](../README.md#code-signing-policy)).
Check [the current requirements](https://signpath.org/apply) before applying.
The publisher Windows then shows is SignPath Foundation.

1. **Apply** at signpath.org as the maintainer. This is a person's step.
2. **In SignPath**, create the project with slug `syn` and a signing policy
   `release-signing` that needs the maintainer's approval. Add the predefined
   GitHub.com trusted build system to the organization and link it to the
   project. Give the project this artifact configuration (the uploaded artifact
   is a zip of the installer):

   ```xml
   <artifact-configuration xmlns="http://signpath.io/artifact-configuration/v1">
     <zip-file>
       <pe-file path="*-setup.exe">
         <authenticode-sign/>
       </pe-file>
     </zip-file>
   </artifact-configuration>
   ```

3. **In the repository** (Settings, Secrets and variables, Actions): add the
   secret `SIGNPATH_API_TOKEN` (an API token of a user who may submit requests)
   and the variable `SIGNPATH_ORGANIZATION_ID`. If the slugs differ from `syn`
   and `release-signing`, change them in the workflow.
4. **Add the attribution** SignPath asks for to the README's code signing
   policy, once approved. It is not there now because it would not be true yet.
5. **Tag a release.** The run uploads the installer, waits for the maintainer's
   approval in SignPath, downloads the signed file and checks it.

## What signing will and will not do

A signature replaces "Unknown publisher" with a name, and gives SmartScreen
something to build a reputation on. It does not end the warning at once:
SmartScreen weighs the publisher and the hash of the file, so a new build can
still be flagged as unrecognized for a while, and how much an expensive
"EV" certificate helps with that is disputed. Keep the instructions for
getting past the prompt in the release notes after signing is on.

## Other ways to sign

Any other signing route replaces the SignPath steps in the workflow and
nothing else: Microsoft's Trusted Signing (individuals in the US and Canada;
organizations in the US, Canada, the EU and the UK), or a paid certificate
whose key lives in a hardware module or a cloud signing service (the key must
be on certified hardware since June 2023). The rest, including the check that
the result is `Valid`, stays.
