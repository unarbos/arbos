"""Run Docker recipe shell against temporary files and a no-network npm double.

Usage: python3 image/tests/test_runtime_packaging.py
"""

from pathlib import Path
import json
import os
import shlex
import shutil
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]


def generated_boot(recipe):
    start = recipe.index("RUN printf '%s\\n'") + 4
    block = recipe[start:recipe.index("\nENV ", start)].replace("\\\n", "")
    words = shlex.split(block)
    return "\n".join(words[2:words.index(">")]) + "\n"


def dependency_layer(recipe):
    start = recipe.index("RUN --mount=type=bind,source=harness/arbos-pi-ext/package.json")
    block = recipe[start:recipe.index("\n\n", start)]
    return block[block.index("\\\n") + 2:].replace("\\\n", "")


def seed_links(recipe):
    start = recipe.index("RUN mkdir -p /etc/arbos/pi-seed/extensions/arbos-pi-ext/node_modules") + 4
    return recipe[start:recipe.index("\nCOPY ", start)].replace("\\\n", "")


class RuntimePackagingTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="arbos-runtime-packaging-")
        self.addCleanup(self.temp.cleanup)
        self.path = Path(self.temp.name)
        self.opt = self.path / "opt"
        self.extension = self.opt / "arbos-pi-ext"
        self.extension.mkdir(parents=True)
        self.manifest = self.extension / "package.json"
        self.pin("9.8.7")
        self.shared = self.opt / "pi-ext-runtime/node_modules/effect"
        self.log = self.path / "npm-calls.jsonl"
        self.boot = generated_boot((ROOT / "image/Dockerfile.dev-inline").read_text())
        full_recipe = (ROOT / "image/Dockerfile").read_text()
        self.full = dependency_layer(full_recipe)
        self.seed_links = seed_links(full_recipe)
        tools = self.path / "bin"
        tools.mkdir()
        global_pi = self.path / "global"
        typebox = global_pi / "@earendil-works/pi-coding-agent/node_modules/typebox"
        typebox.mkdir(parents=True)
        (typebox / "package.json").write_text('{}')
        npm = tools / "npm"
        npm.write_text("""#!/usr/bin/env python3
from pathlib import Path
import json, os, sys
args = sys.argv[1:]
if args == ['root', '-g']:
    print(os.environ['TEST_GLOBAL_PI'])
elif args == ['cache', 'clean', '--force']:
    pass
else:
    assert args[0] == 'install'
    for flag in ['--ignore-scripts', '--no-package-lock', '--no-save', '--no-audit', '--no-fund']:
        assert flag in args, args
    prefix = Path(args[args.index('--prefix') + 1])
    version = args[-1].removeprefix('effect@')
    package = prefix / 'node_modules/effect'
    package.mkdir(parents=True, exist_ok=True)
    (package / 'package.json').write_text(json.dumps({'name': 'effect', 'version': version, 'type': 'module', 'exports': './index.js'}))
    (package / 'index.js').write_text('export const installedVersion = ' + json.dumps(version) + ';')
    with open(os.environ['TEST_NPM_LOG'], 'a') as out:
        out.write(json.dumps(args) + '\\n')
""")
        npm.chmod(0o755)
        self.env = dict(os.environ, TEST_NPM_LOG=str(self.log), TEST_GLOBAL_PI=str(global_pi))
        self.env["PATH"] = str(tools) + os.pathsep + self.env["PATH"]

    def pin(self, version):
        self.manifest.write_text(json.dumps({"dependencies": {"effect": version}}))

    def checked(self, command):
        result = subprocess.run(command, env=self.env, text=True, capture_output=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        return result

    def install_count(self):
        return len(self.log.read_text().splitlines()) if self.log.exists() else 0

    def bootstrap(self, boot=None):
        boot = self.boot if boot is None else boot
        snippet = boot[boot.index("EFFECT_VERSION="):boot.index("export ARBOS_PI_EXT=")]
        script = self.path / "boot.sh"
        script.write_text("set -e\n" + snippet.replace("/opt/arbos", shlex.quote(str(self.opt))))
        return subprocess.run(["bash", str(script)], env=self.env, text=True, capture_output=True)

    def assert_shared_link(self, extension):
        link = extension / "node_modules/effect"
        self.assertTrue(link.is_symlink(), "workspace must link Effect, not copy its package tree")
        self.assertEqual(link.resolve(), self.shared.resolve())

    def import_fixture(self, extension, version):
        probe = extension / "probe.mjs"
        probe.write_text("import { installedVersion } from 'effect'; "
                         + f"if (installedVersion !== {json.dumps(version)}) process.exit(1);")
        return subprocess.run(["node", str(probe)], env=self.env, text=True, capture_output=True)

    def test_generated_recipe_shell_parses(self):
        for name, source in [("boot", self.boot), ("full", self.full), ("seed", self.seed_links)]:
            with self.subTest(recipe=name):
                script = self.path / f"{name}.sh"
                script.write_text(source)
                self.checked(["bash", "-n", str(script)])

    def test_cold_boot_installs_once_and_replaces_bundled_dependency_tree(self):
        (self.extension / "node_modules/effect/stale").mkdir(parents=True)
        result = self.bootstrap()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.install_count(), 1)
        self.assert_shared_link(self.extension)
        self.assertFalse((self.extension / "node_modules/effect/stale").exists())
        self.assertEqual(self.import_fixture(self.extension, "9.8.7").returncode, 0)

    def test_warm_boot_reuses_the_matching_version(self):
        for _ in range(2):
            result = self.bootstrap()
            self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.install_count(), 1)
        self.assert_shared_link(self.extension)

    def test_changed_manifest_refreshes_one_shared_runtime(self):
        self.assertEqual(self.bootstrap().returncode, 0)
        self.pin("9.8.8")
        result = self.bootstrap()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.install_count(), 2)
        self.assertEqual(json.loads((self.shared / "package.json").read_text())["version"], "9.8.8")
        self.assert_shared_link(self.extension)

    def test_unpinned_or_remote_specs_fail_before_npm(self):
        for version in [None, "^9.8.8", "https://example.invalid/effect.tgz", "file:/tmp/other"]:
            with self.subTest(version=version):
                self.pin(version)
                self.assertNotEqual(self.bootstrap().returncode, 0)
                self.assertEqual(self.install_count(), 0)

    def test_workspace_copies_keep_shared_links_and_resolve_esm(self):
        self.assertEqual(self.bootstrap().returncode, 0)
        for project in ["project-a", "project-b"]:
            with self.subTest(project=project):
                destination = self.path / project / "extensions/arbos-pi-ext"
                shutil.copytree(self.extension, destination, symlinks=True)
                self.assert_shared_link(destination)
                self.assertFalse(any(p.is_file() for p in (destination / "node_modules").iterdir()))
                result = self.import_fixture(destination, "9.8.7")
                self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.install_count(), 1)

    def test_full_image_layer_uses_manifest_pin_and_disables_lifecycle_scripts(self):
        library = self.path / "full-runtime"
        source = self.full.replace("/tmp/arbos-pi-ext-package.json", shlex.quote(str(self.manifest)))
        source = source.replace("/usr/local/lib/arbos/pi-ext-runtime", shlex.quote(str(library)))
        script = self.path / "full.sh"
        script.write_text("set -e\n" + source)
        self.checked(["bash", str(script)])
        self.assertEqual(self.install_count(), 1)
        package = library / "node_modules/effect/package.json"
        self.assertEqual(json.loads(package.read_text())["version"], "9.8.7")
        extension = self.path / "full-seed/extensions/arbos-pi-ext"
        links = self.seed_links.replace("/etc/arbos/pi-seed/extensions/arbos-pi-ext", shlex.quote(str(extension)))
        links = links.replace("/usr/local/lib/arbos/pi-ext-runtime", shlex.quote(str(library)))
        script.write_text("set -e\n" + links)
        self.checked(["bash", str(script)])
        link = extension / "node_modules/effect"
        self.assertTrue(link.is_symlink())
        self.assertEqual(link.resolve(), (library / "node_modules/effect").resolve())
        result = self.import_fixture(extension, "9.8.7")
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_missing_effect_link_negative_control_is_detected(self):
        link = "ln -sfn /opt/arbos/pi-ext-runtime/node_modules/effect /opt/arbos/arbos-pi-ext/node_modules/effect"
        self.assertEqual(self.boot.count(link), 1)
        mutant = self.boot.replace(link + "\n", "")
        result = self.bootstrap(mutant)
        self.assertEqual(result.returncode, 0, result.stderr)
        with self.assertRaises(AssertionError):
            self.assert_shared_link(self.extension)
        result = self.import_fixture(self.extension, "9.8.7")
        self.assertNotEqual(result.returncode, 0, "removing the link must prevent extension imports")
        self.assertIn("ERR_MODULE_NOT_FOUND", result.stderr)

    def test_installed_effect_subpaths_resolve_from_two_workspace_links(self):
        package = ROOT / "harness/arbos-pi-ext/node_modules/effect"
        if not (package / "package.json").exists():
            self.skipTest("run pnpm install in harness for the real Effect import check")
        package = package.resolve()
        manifest = json.loads((ROOT / "harness/arbos-pi-ext/package.json").read_text())
        self.assertEqual(json.loads((package / "package.json").read_text())["version"], manifest["dependencies"]["effect"])
        self.shared.parent.mkdir(parents=True)
        self.shared.symlink_to(package, target_is_directory=True)
        for project in ["real-a", "real-b"]:
            with self.subTest(project=project):
                extension = self.path / project
                (extension / "node_modules").mkdir(parents=True)
                (extension / "node_modules/effect").symlink_to(self.shared, target_is_directory=True)
                self.assert_shared_link(extension)
                probe = extension / "probe.mjs"
                probe.write_text('import * as Effect from "effect/Effect";\n'
                                 'import * as Schema from "effect/Schema";\n'
                                 'const value = Schema.decodeUnknownSync(Schema.Struct({ ok: Schema.Boolean }))({ ok: true });\n'
                                 'if (Effect.runSync(Effect.succeed(value.ok)) !== true) process.exit(1);\n')
                self.checked(["node", str(probe)])


if __name__ == "__main__":
    unittest.main(verbosity=2)
