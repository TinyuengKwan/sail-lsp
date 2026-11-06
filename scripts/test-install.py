#!/usr/bin/env python3
"""Exercise installed executables and real bootstrap/lifecycle transactions.

Version fixtures are native executables built with rustc; the production archive
is additionally exercised through a real LSP connection. No user config is used.
"""
import argparse
import hashlib
import io
import json
import os
from pathlib import Path
import queue
import shutil
import signal
import subprocess
import sys
import tarfile
import tempfile
import threading
import time

sys.path.insert(0, str(Path(__file__).parent))
import importlib.util
_spec = importlib.util.spec_from_file_location("packaging_sail", Path(__file__).with_name("package-distribution.py"))
packaging = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(packaging)
ROOT = Path(__file__).resolve().parent.parent


def run(args, *, ok=True, env=None, timeout=120, cwd=None):
    result = subprocess.run([str(a) for a in args], text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env, timeout=timeout, cwd=cwd)
    if (result.returncode == 0) != ok:
        raise AssertionError(f"Unexpected status {result.returncode}: {args}\n{result.stdout}\n{result.stderr}")
    return result


def metadata(binary):
    return json.loads(run([binary, "--version", "--json"]).stdout)


def bootstrap(args):
    if os.name == "nt":
        return ["powershell.exe", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", ROOT / "install.ps1", *args]
    return ["sh", ROOT / "install.sh", *args]


def lsp_smoke(binary, directory, *, command=None, workspace_uri=None):
    workspace = directory / "workspace"
    workspace.mkdir()
    (workspace / "sail.proj").write_text("Main { files main.sail }\n")
    source = "function answer() -> int = 42\n"
    (workspace / "main.sail").write_text(source)
    uri = workspace_uri or workspace.as_uri()
    file_uri = uri + "/main.sail"
    process = subprocess.Popen(command or [str(binary)], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    messages = queue.Queue()
    stderr = []
    def read_protocol():
        try:
            while True:
                headers = {}
                while True:
                    line = process.stdout.readline()
                    if not line:
                        return
                    if line == b"\r\n":
                        break
                    key, value = line.decode().split(":", 1)
                    headers[key.lower()] = value.strip()
                messages.put(json.loads(process.stdout.read(int(headers["content-length"]))))
        except BaseException as error:
            messages.put(error)
    def drain():
        stderr.append(process.stderr.read().decode(errors="replace"))
    reader = threading.Thread(target=read_protocol, daemon=True)
    errors = threading.Thread(target=drain, daemon=True)
    reader.start(); errors.start()
    def send(method, params, identity=None):
        value = {"jsonrpc":"2.0", "method":method, "params":params}
        if identity is not None:
            value["id"] = identity
        data = json.dumps(value).encode()
        process.stdin.write(f"Content-Length: {len(data)}\r\n\r\n".encode()+data)
        process.stdin.flush()
    def until(predicate):
        deadline = time.monotonic()+45
        while time.monotonic() < deadline:
            message = messages.get(timeout=max(0.1, deadline-time.monotonic()))
            if isinstance(message, BaseException):
                raise message
            if "method" in message and "id" in message:
                response = json.dumps({"jsonrpc":"2.0", "id":message["id"], "result":None}).encode()
                process.stdin.write(f"Content-Length: {len(response)}\r\n\r\n".encode()+response);process.stdin.flush()
            if predicate(message):
                return message
        raise AssertionError("LSP response timeout")
    try:
        send("initialize", {"processId":None,"capabilities":{},"workspaceFolders":[{"uri":uri,"name":"install-test"}]}, 1)
        assert until(lambda m:m.get("id")==1)["result"]["capabilities"]["hoverProvider"]
        send("initialized", {})
        send("textDocument/didOpen", {"textDocument":{"uri":file_uri,"languageId":"sail","version":1,"text":source}})
        until(lambda m:m.get("method")=="sail-lsp/serverStatus" and m["params"].get("quiescent"))
        send("textDocument/hover", {"textDocument":{"uri":file_uri},"position":{"line":0,"character":11}}, 2)
        result = until(lambda m:m.get("id")==2)
        assert result.get("result") and "answer" in json.dumps(result), result
        send("shutdown", None, 3);assert "error" not in until(lambda m:m.get("id")==3)
        send("exit", None);process.wait(timeout=10)
        assert process.returncode==0, stderr
    finally:
        if process.poll() is None:
            process.kill();process.wait()
        reader.join(timeout=3);errors.join(timeout=3)


def fixture(root, meta, version):
    data={**meta,"version":version,"revision":"0123456789abcdef"+version.replace(".",""),"solver":"z3-vendored"}
    source=root/f"fixture-{version}.rs"
    source.write_text('fn main() { let args: Vec<_> = std::env::args().collect(); if args.get(1).map(String::as_str)==Some("--version") { println!("{}", r###"'+json.dumps(data)+'"###); } else if args.get(1).map(String::as_str)==Some("doctor") { println!("{{\\\"ok\\\":true}}"); } else { std::process::exit(2); } }')
    binary=root/(f"fixture-{version}"+(".exe" if os.name=="nt" else ""))
    run(["rustc","--edition=2021","--target",meta["target"],"--crate-name","sail_install_fixture",*(["-C","target-feature=+crt-static"] if os.name=="nt" else []),str(source),"-o",str(binary)])
    return packaging.package(binary,root/"fixtures"/version,f"v{version}",allow_dirty=True)


def lifecycle(server, archive, vsix):
    results=[]
    with tempfile.TemporaryDirectory(prefix="sail-install-test-") as temp:
        root=Path(temp).resolve();prefix=root/"install space ' apostrophe";config=root/"managed config";editors=root/"editor config"
        base=["--prefix",prefix,"--config-home",config,"--editor-root",editors,"--no-modify-path"]
        options=[server,"install",*base]
        meta=metadata(server);binary=prefix/"bin"/("sail-lsp.exe" if os.name=="nt" else "sail-lsp")
        if archive:
            args=[*base,"--archive",archive,"--editor","neovim,zed,helix,emacs"]
            if vsix:
                cli=root/("code.cmd" if os.name=="nt" else "code")
                cli.write_text("@exit /b 0\r\n" if os.name=="nt" else "#!/bin/sh\nexit 0\n")
                cli.chmod(0o755);args += ["--editor","vscode","--vsix",vsix,"--code-command",cli]
            run(bootstrap(args),timeout=180)
            assert metadata(binary)==meta
            assert json.loads(run([binary,"doctor","--json"]).stdout)["ok"]
            broken=root/"broken-registration";broken.mkdir()
            (broken/"vscode.json").write_text('{"schemaVersion":1,"path":"/missing/sail-lsp"}')
            env={**os.environ,"SAIL_LSP_CONFIG_HOME":str(broken)}
            diagnosis=json.loads(run([binary,"doctor","--json"],ok=False,env=env).stdout)
            assert not diagnosis["ok"] and any(c["name"]=="vscodeRegistration" and c["status"]=="error" for c in diagnosis["checks"])
            lsp_smoke(binary,root)
            if sys.platform == "linux":
                tool_dir=root/"slow-tools";tool_dir.mkdir()
                compiler=tool_dir/"sail"
                compiler.write_text('#!/bin/sh\nsleep 60 &\necho "$!" > "$SAIL_LSP_TEST_CHILD"\nwait\n')
                compiler.chmod(0o755)
                pid_file=root/"compiler-child.pid"
                env={**os.environ,"PATH":str(tool_dir)+os.pathsep+os.environ.get("PATH",""),"SAIL_LSP_TEST_CHILD":str(pid_file)}
                report=json.loads(run([binary,"doctor","--json"],env=env,timeout=20).stdout)
                assert any(c["name"]=="upstreamSail" and c["status"]=="warning" for c in report["checks"])
                child=pid_file.read_text().strip();proc=Path("/proc")/child/"stat"
                if proc.exists():
                    state=proc.read_text().rsplit(")",1)[1].strip().split()[0]
                    if state != "Z":
                        os.kill(int(child),9)
                        raise AssertionError("Timed-out compiler left a running descendant")
                pid_file.unlink()
                interrupted=subprocess.Popen([str(binary),"doctor","--json"],env=env,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True)
                try:
                    deadline=time.monotonic()+10
                    while not pid_file.exists() and time.monotonic()<deadline:
                        time.sleep(0.01)
                    assert pid_file.exists(), "Optional compiler never started"
                    interrupted.send_signal(signal.SIGINT)
                    stdout,stderr=interrupted.communicate(timeout=10)
                    assert interrupted.returncode != 0 and "cancelled" in stderr, (stdout,stderr)
                    child=pid_file.read_text().strip();proc=Path("/proc")/child/"stat"
                    if proc.exists() and proc.read_text().rsplit(")",1)[1].strip().split()[0] != "Z":
                        os.kill(int(child),9)
                        raise AssertionError("Interrupted command left a running descendant")
                finally:
                    if interrupted.poll() is None:
                        interrupted.kill();interrupted.wait()
            assert (editors/"nvim/plugin/sail_lsp_managed.lua").is_file()
            assert (prefix/"editors/zed/extension.toml").is_file()
            assert json.loads((editors/"zed/settings.json").read_text())["lsp"]["sail_lsp"]["binary"]["path"]==str(binary)
            if vsix:
                assert json.loads((config/"vscode.json").read_text())["path"]==str(binary)
            run(bootstrap([*base,"--uninstall"]))
            assert not binary.exists()
            assert not (editors/"nvim/plugin/sail_lsp_managed.lua").exists()
            assert not (config/"vscode.json").exists()
            assert not (editors/"helix/languages.toml").exists()
            assert not (editors/"zed/settings.json").exists()
            results.append("production bootstrap, doctor, LSP hover/shutdown, editor setup and uninstall")
            mirror=root/"mirror"
            release=mirror/"download"/("v"+meta["version"]);release.mkdir(parents=True)
            for asset in [archive,Path(str(archive)+".sha256")]:
                shutil.copyfile(asset,release/asset.name)
            online_args=[*base,"--version","v"+meta["version"],"--release-base-url","file://"+str(mirror)]
            run(bootstrap(online_args),timeout=180)
            assert metadata(binary)==meta
            run(bootstrap([*base,"--uninstall"]))
            run([server,"install",*online_args],timeout=180)
            assert metadata(binary)==meta
            run(bootstrap([*base,"--uninstall"]))
            results.append("pinned release download layout through an offline mirror, bootstrap and native CLI")
        first=fixture(root,meta,"1.0.0");second=fixture(root,meta,"1.1.0")
        run([*options,"--archive",first]);assert metadata(binary)["version"]=="1.0.0"
        state=prefix/"install-state.json";before=state.read_bytes()
        run([*options,"--archive",second,"--sha256","0"*64],ok=False)
        assert state.read_bytes()==before and metadata(binary)["version"]=="1.0.0"
        run([*options,"--archive",first]);assert state.read_bytes()==before
        if os.name != "nt":
            import fcntl
            with (prefix/".install.lock").open("r+") as held:
                fcntl.flock(held,fcntl.LOCK_EX | fcntl.LOCK_NB)
                error=run([*options,"--archive",second],ok=False)
                assert "Another Sail installation" in error.stderr
                assert state.read_bytes()==before
        run([*options,"--archive",second]);assert metadata(binary)["version"]=="1.1.0"
        run([*options,"--rollback"]);assert metadata(binary)["version"]=="1.0.0"
        run([*options,"--rollback"]);assert metadata(binary)["version"]=="1.1.0"
        assert "1.0.0" in run([*options,"--list"]).stdout
        results.append("native version fixtures: repeat install, failed checksum preserves active version, upgrade, two-way rollback")
        # A matching hash does not make archive paths or symlinks safe.
        malicious=root/"unsafe.tar.gz"
        with tarfile.open(malicious,"w:gz") as tar:
            entry=tarfile.TarInfo("../outside");entry.size=1;tar.addfile(entry,io.BytesIO(b"x"))
        before=state.read_bytes()
        run([*options,"--archive",malicious,"--sha256",packaging.digest(malicious)],ok=False)
        assert not (root/"outside").exists() and state.read_bytes()==before
        with tarfile.open(malicious,"w:gz") as tar:
            entry=tarfile.TarInfo("link");entry.type=tarfile.SYMTYPE;entry.linkname=str(root);tar.addfile(entry)
        run([*options,"--archive",malicious,"--sha256",packaging.digest(malicious)],ok=False)
        assert state.read_bytes()==before
        if os.name != "nt":
            run(bootstrap([*base,"--archive",malicious,"--sha256",packaging.digest(malicious)]),ok=False)
            assert state.read_bytes()==before
        run([*options,"--archive",first,"--version","v9.0.0"],ok=False)
        assert state.read_bytes()==before
        results.append("unsafe path/link rejection and pinned version mismatch preserve installed state")
        if os.name != "nt":
            identity=json.loads(state.read_text())["active"]
            owned=prefix/"versions"/identity;external=root/"moved-owned-directory"
            owned.rename(external);owned.symlink_to(external,target_is_directory=True)
            before=state.read_bytes()
            error=run([*options,"--uninstall"],ok=False)
            assert "symlink" in error.stderr and state.read_bytes()==before
            assert (external/"sail-lsp").is_file()
            owned.unlink();external.rename(owned)
            results.append("concurrent installer lock and changed directory symlink are rejected without deleting files")
        unrelated=prefix/"personal-note.txt";unrelated.write_text("keep")
        run([*options,"--uninstall"]);assert unrelated.read_text()=="keep"
        assert not binary.exists() and not state.exists()
        results.append("uninstall preserves unrelated files")
        if os.name!="nt":
            profile=root/"custom profile";profile.write_text("# user configuration\n")
            config_base=["--prefix",prefix,"--config-home",config,"--editor-root",editors,"--profile",profile]
            # Remove fresh-install editor documents left from earlier use.
            shutil.rmtree(editors,ignore_errors=True);shutil.rmtree(config,ignore_errors=True)
            zed=editors/"zed/settings.json";zed.parent.mkdir(parents=True)
            zed.write_text('{\n // keep comment\n "theme":"original",\n "lsp":{"other":{"setting":true}},\n}\n')
            dotfile=root/"zed-dotfile.json";zed.rename(dotfile);zed.symlink_to(dotfile)
            helix=editors/"helix/languages.toml";helix.parent.mkdir(parents=True)
            helix.write_text('# keep comment\n[[language]]\nname="rust"\nlanguage-servers=["rust-analyzer"]\n')
            run([server,"install",*config_base,"--archive",first,"--editor","zed,neovim,helix,emacs"])
            profile.write_text(profile.read_text()+"# added between upgrades\n")
            zed.write_text(zed.read_text().replace('"original"','"new-theme"'))
            run([server,"install",*config_base,"--archive",second,"--editor","zed,neovim,helix,emacs"])
            expected=zed.read_text();zed.write_text(expected.replace(str(binary),"/custom/sail-lsp"))
            edited=zed.read_bytes()
            run([server,"install",*config_base,"--archive",first,"--editor","zed"],ok=False)
            assert zed.read_bytes()==edited and metadata(binary)["version"]=="1.1.0"
            zed.write_text(expected)
            nvim=editors/"nvim/plugin/sail_lsp_managed.lua";nvim.write_text(nvim.read_text()+"-- user addition\n")
            run([server,"install",*config_base,"--archive",first,"--editor","neovim"],ok=False)
            assert metadata(binary)["version"]=="1.1.0"
            run([server,"install",*config_base,"--uninstall"])
            assert profile.read_text()=="# user configuration\n# added between upgrades\n"
            assert zed.is_symlink() and dotfile.is_file()
            assert "new-theme" in zed.read_text() and "keep comment" in zed.read_text() and str(binary) not in zed.read_text()
            assert "rust-analyzer" in helix.read_text() and "keep comment" in helix.read_text() and "sail-lsp-managed" not in helix.read_text()
            assert nvim.read_text().endswith("-- user addition\n")
            results.append("upgrade/uninstall retain unrelated profile/JSONC/TOML changes; edited managed files are never overwritten")
            run([server,"install","--prefix",prefix,"--config-home",config,
                 "--editor-root","relative-editors","--profile","relative-profile",
                 "--archive",first,"--editor","neovim,neovim"],cwd=root)
            elsewhere=root/"another-directory";elsewhere.mkdir()
            run([server,"install","--prefix",prefix,"--uninstall"],cwd=elsewhere)
            assert not (root/"relative-editors/nvim/plugin/sail_lsp_managed.lua").exists()
            assert not (root/"relative-profile").exists()
            results.append("relative integration paths survive a working-directory change; duplicate editor selections are idempotent")
    return results


def source_check(checkout,revision):
    with tempfile.TemporaryDirectory(prefix="sail-source-install-") as temp:
        root=Path(temp);prefix=root/"install";config=root/"config"
        code=root/("code.cmd" if os.name=="nt" else "code")
        code.write_text("@exit /b 0\r\n" if os.name=="nt" else "#!/bin/sh\nexit 0\n")
        code.chmod(0o755)
        args=["--path",checkout,"--rev",revision,"--prefix",prefix,"--config-home",config,
              "--editor-root",root/"editors","--editor","vscode,zed,neovim,helix,emacs",
              "--code-command",code,"--no-modify-path"]
        run(bootstrap(args),timeout=3600)
        binary=prefix/"bin"/("sail-lsp.exe" if os.name=="nt" else "sail-lsp")
        info = metadata(binary)
        assert info["solver"]=="z3-vendored"
        assert info["revision"] == revision, info
        assert (root/"editors/nvim/plugin/sail_lsp_managed.lua").is_file()
        assert (prefix/"editors/zed/extension.toml").is_file()
        assert json.loads((config/"vscode.json").read_text())["path"]==str(binary)
        lsp_smoke(binary,root)
        before=(prefix/"install-state.json").read_bytes()
        run(bootstrap(args),timeout=3600)
        assert (prefix/"install-state.json").read_bytes()==before
        run(bootstrap(["--prefix",prefix,"--config-home",config,"--no-modify-path","--uninstall"]))
    return ["clean pinned checkout source bootstrap, vendored solver, source VSIX with CLI stub, all editor configuration, real LSP and uninstall"]


def clean_container(folder):
    archive=next(folder.rglob("sail-lsp-x86_64-unknown-linux-gnu.tar.gz")).resolve()
    with tempfile.TemporaryDirectory(prefix="sail-clean-container-") as temp:
        root=Path(temp);dockerfile=root/"Dockerfile"
        dockerfile.write_text("FROM ubuntu:22.04\nRUN apt-get update && apt-get install -y --no-install-recommends libstdc++6 ca-certificates && rm -rf /var/lib/apt/lists/*\n")
        run(["docker","build","-t","sail-lsp-clean-install-test",root],timeout=300)
        (root/"home").mkdir()
        docker_args=["docker","run","--rm","--network","none","--user",f"{os.getuid()}:{os.getgid()}",
                     "--env","HOME=/installed/home","-v",f"{ROOT}:/repo:ro",
                     "-v",f"{archive.parent}:/assets:ro","-v",f"{root}:/installed"]
        commands="""set -eu
! command -v cargo
! command -v node
! command -v python3
! command -v z3
sh /repo/install.sh --archive /assets/ARCHIVE --prefix /installed/sail --no-modify-path
/installed/sail/bin/sail-lsp doctor --json > /installed/doctor.json
""".replace("ARCHIVE",archive.name)
        run([*docker_args,"sail-lsp-clean-install-test","sh","-c",commands],timeout=120)
        assert json.loads((root/"doctor.json").read_text())["ok"]
        lsp_smoke(None,root,command=[*docker_args,"-i","sail-lsp-clean-install-test","/installed/sail/bin/sail-lsp"],workspace_uri="file:///installed/workspace")
        run([*docker_args,"sail-lsp-clean-install-test","sh","/repo/install.sh","--prefix","/installed/sail","--no-modify-path","--uninstall"])
    return ["Ubuntu 22.04 container, no Rust/Node/Python/Z3 or runtime network; bootstrap/doctor/real LSP/uninstall"]


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument("--server",type=Path);p.add_argument("--archive",type=Path);p.add_argument("--vsix",type=Path)
    p.add_argument("--source-checkout",type=Path);p.add_argument("--revision");p.add_argument("--clean-container",type=Path)
    p.add_argument("--output",type=Path,required=True)
    args=p.parse_args();report={"status":"running","platform":sys.platform,"tests":[]}
    try:
        if args.source_checkout:
            if not args.revision:
                p.error("--revision is required with --source-checkout")
            report["tests"]=source_check(args.source_checkout.resolve(),args.revision)
        elif args.clean_container:
            report["tests"]=clean_container(args.clean_container)
        else:
            if not args.server:
                p.error("--server is required")
            report["tests"]=lifecycle(args.server.resolve(),args.archive.resolve() if args.archive else None,args.vsix.resolve() if args.vsix else None)
        report["status"]="passed"
    except BaseException as error:
        report["status"]="failed";report["error"]=repr(error);raise
    finally:
        args.output.parent.mkdir(parents=True,exist_ok=True);args.output.write_text(json.dumps(report,indent=2)+"\n")
        print(json.dumps(report,indent=2))


if __name__=="__main__":
    main()
