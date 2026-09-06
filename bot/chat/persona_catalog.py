"""Strict loading for deployment-owned major persona bundles."""

import logging
import re
import stat
from collections.abc import Mapping
from dataclasses import dataclass, replace
from pathlib import Path
from types import MappingProxyType
from typing import Literal

import yaml

from . import persona as _persona_module
from .persona import PERSONA_DIR
from .progress import (
    StagingConfigError,
    StagingLines,
    parse_complete_staging,
    parse_profile_staging,
)

log = logging.getLogger(__name__)

PERSONA_ROOT = PERSONA_DIR
MANIFEST_NAME = "personas.yaml"
EXAMPLE_ID = "kanade"
_ID_RE = re.compile(r"^[a-z0-9][a-z0-9-]{0,49}$")
_MANIFEST_KEYS = {"schema_version", "default", "personas"}
_DESCRIPTOR_KEYS = {"id", "label", "aliases"}
_OPTIONAL_DESCRIPTOR_KEYS = {"identity", "behaviour", "staging"}
_MAX_FILENAME_LENGTH = 100
_MAX_PERSONAS = 100
_MAX_ALIASES = 20
_MAX_LABEL_LENGTH = 200
_MAX_ALIAS_LENGTH = 100


class PersonaCatalogError(ValueError):
    """A deployment configuration error safe to present to operators."""


@dataclass(frozen=True)
class PersonaDescriptor:
    id: str
    label: str
    aliases: tuple[str, ...] = ()
    identity: str = "identity.md"
    behaviour: str = "default.md"
    staging: str = "staging.yaml"


@dataclass(frozen=True)
class PersonaBundleSource:
    id: str
    identity_path: Path
    default_behaviour_path: Path
    staging_path: Path


@dataclass(frozen=True)
class PersonaBundle:
    id: str
    label: str
    identity: str
    default_behaviour: str
    staging: StagingLines
    source: PersonaBundleSource
    fell_back: bool = False
    issue: str | None = None


@dataclass(frozen=True)
class PersonaRuntime:
    bundle: PersonaBundle
    profile_staging: Mapping[str, StagingLines]


@dataclass(frozen=True)
class PersonaCatalog:
    default_id: str
    descriptors: tuple[PersonaDescriptor, ...]
    mode: Literal["manifest", "legacy"]
    root: Path
    issues: Mapping[str, str]

    def resolve(self, selection: str | None) -> PersonaDescriptor | None:
        """Resolve an exact canonical ID or legacy alias; never interpret paths."""
        if not selection:
            return None
        for descriptor in self.descriptors:
            if selection == descriptor.id or selection in descriptor.aliases:
                return descriptor
        return None

    @property
    def choices(self) -> tuple[PersonaDescriptor, ...]:
        return tuple(item for item in self.descriptors if item.id not in self.issues)


def _safe_issue(identifier: str, component: str | None = None) -> str:
    if component:
        return f"persona {identifier!r} has an invalid {component}"
    return f"persona {identifier!r} is invalid"


def _load_yaml(path: Path, what: str) -> object:
    try:
        return yaml.safe_load(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, yaml.YAMLError) as exc:
        raise PersonaCatalogError(f"{what} is unreadable or invalid") from exc


def _validate_alias(value: object) -> str:
    if not isinstance(value, str) or not value or len(value) > _MAX_ALIAS_LENGTH:
        raise PersonaCatalogError("manifest alias must be a non-empty bounded string")
    if (
        value in {".", ".."}
        or "\x00" in value
        or "/" in value
        or "\\" in value
        or Path(value).is_absolute()
    ):
        raise PersonaCatalogError("manifest alias must not be path-shaped")
    return value


def _validate_bundle_filename(value: object, field: str, suffixes: set[str]) -> str:
    if (
        not isinstance(value, str)
        or not value
        or len(value) > _MAX_FILENAME_LENGTH
        or value in {".", ".."}
        or "\x00" in value
        or "/" in value
        or "\\" in value
        or Path(value).is_absolute()
    ):
        raise PersonaCatalogError(f"persona descriptor {field} must be a bare filename")
    if Path(value).suffix not in suffixes:
        raise PersonaCatalogError(f"persona descriptor {field} has an unsupported extension")
    return value


def _parse_manifest(path: Path) -> tuple[str, tuple[PersonaDescriptor, ...]]:
    data = _load_yaml(path, "persona manifest")
    if not isinstance(data, dict) or set(data) != _MANIFEST_KEYS:
        raise PersonaCatalogError(
            "persona manifest must contain exactly schema_version, default, and personas"
        )
    if data["schema_version"] != 1:
        raise PersonaCatalogError("persona manifest has an unsupported schema_version")
    records = data["personas"]
    if not isinstance(records, list) or not records or len(records) > _MAX_PERSONAS:
        raise PersonaCatalogError("persona manifest personas must be a non-empty bounded list")
    descriptors: list[PersonaDescriptor] = []
    ids: set[str] = set()
    aliases: set[str] = set()
    for record in records:
        keys = set(record) if isinstance(record, dict) else set()
        if not isinstance(record, dict) or not _DESCRIPTOR_KEYS <= keys:
            raise PersonaCatalogError("persona descriptor has unknown or missing keys")
        if not keys <= (_DESCRIPTOR_KEYS | _OPTIONAL_DESCRIPTOR_KEYS):
            raise PersonaCatalogError("persona descriptor has unknown or missing keys")
        identifier = record["id"]
        label = record["label"]
        raw_aliases = record["aliases"]
        if not isinstance(identifier, str) or not _ID_RE.fullmatch(identifier) or identifier in ids:
            raise PersonaCatalogError("persona manifest has an invalid or duplicate ID")
        if not isinstance(label, str) or not label.strip() or len(label) > _MAX_LABEL_LENGTH:
            raise PersonaCatalogError("persona descriptor label must be non-empty and bounded")
        if not isinstance(raw_aliases, list) or len(raw_aliases) > _MAX_ALIASES:
            raise PersonaCatalogError("persona descriptor aliases must be a bounded list")
        parsed_aliases = tuple(_validate_alias(alias) for alias in raw_aliases)
        if len(set(parsed_aliases)) != len(parsed_aliases):
            raise PersonaCatalogError("persona manifest has duplicate aliases")
        if any(alias in ids or alias in aliases or alias == identifier for alias in parsed_aliases):
            raise PersonaCatalogError("persona manifest aliases collide with an ID or alias")
        ids.add(identifier)
        aliases.update(parsed_aliases)
        identity = record.get("identity", "identity.md")
        behaviour = record.get("behaviour", "default.md")
        staging = record.get("staging", "staging.yaml")
        descriptors.append(
            PersonaDescriptor(
                identifier,
                label.strip(),
                parsed_aliases,
                _validate_bundle_filename(identity, "identity", {".md"}),
                _validate_bundle_filename(behaviour, "behaviour", {".md"}),
                _validate_bundle_filename(staging, "staging", {".yaml", ".yml"}),
            )
        )
    if aliases & ids:
        raise PersonaCatalogError("persona manifest aliases collide with a canonical ID")
    default = data["default"]
    if not isinstance(default, str) or default not in ids:
        raise PersonaCatalogError("persona manifest default must name a known ID")
    return default, tuple(descriptors)


def _required_file(bundle_dir: Path, name: str, identifier: str, root: Path) -> Path:
    if bundle_dir.is_symlink() or not bundle_dir.is_dir() or root.is_symlink():
        raise PersonaCatalogError(_safe_issue(identifier, "bundle directory"))
    try:
        resolved_root = root.resolve()
        resolved_bundle = bundle_dir.resolve()
        resolved_bundle.relative_to(resolved_root)
    except (OSError, ValueError) as exc:
        raise PersonaCatalogError(_safe_issue(identifier, "bundle directory")) from exc
    path = bundle_dir / name
    try:
        mode = path.lstat().st_mode
    except OSError as exc:
        raise PersonaCatalogError(_safe_issue(identifier, name)) from exc
    if path.is_symlink() or not stat.S_ISREG(mode):
        raise PersonaCatalogError(_safe_issue(identifier, name))
    try:
        path.resolve().relative_to(resolved_bundle)
    except ValueError as exc:
        raise PersonaCatalogError(_safe_issue(identifier, name)) from exc
    return path


def _load_bundle(root: Path, descriptor: PersonaDescriptor) -> PersonaBundle:
    personas_dir = root / "personas"
    if personas_dir.is_symlink() or not personas_dir.is_dir():
        raise PersonaCatalogError(_safe_issue(descriptor.id, "personas directory"))
    bundle_dir = personas_dir / descriptor.id
    identity_path = _required_file(bundle_dir, descriptor.identity, descriptor.id, personas_dir)
    behaviour_path = _required_file(bundle_dir, descriptor.behaviour, descriptor.id, personas_dir)
    staging_path = _required_file(bundle_dir, descriptor.staging, descriptor.id, personas_dir)
    try:
        identity = identity_path.read_text(encoding="utf-8").strip()
        behaviour = behaviour_path.read_text(encoding="utf-8").strip()
    except (OSError, UnicodeError) as exc:
        raise PersonaCatalogError(_safe_issue(descriptor.id, "Markdown component")) from exc
    if not identity or not behaviour:
        raise PersonaCatalogError(_safe_issue(descriptor.id, "Markdown component"))
    try:
        staging = parse_complete_staging(_load_yaml(staging_path, "persona staging"), "staging")
    except StagingConfigError as exc:
        raise PersonaCatalogError(_safe_issue(descriptor.id, "staging.yaml")) from exc
    return PersonaBundle(
        descriptor.id,
        descriptor.label,
        identity,
        behaviour,
        staging,
        PersonaBundleSource(descriptor.id, identity_path, behaviour_path, staging_path),
    )


def _legacy_descriptors(root: Path) -> tuple[PersonaDescriptor, ...]:
    paths = [root / "identities", root]
    names: set[str] = set()
    for directory in paths:
        try:
            names.update(
                path.name for path in directory.iterdir() if path.is_file() and path.suffix == ".md"
            )
        except OSError:
            continue
    names.discard("README.md")
    return tuple(PersonaDescriptor(name, name, ()) for name in sorted(names))


def load_catalog(root: str | Path | None = None) -> PersonaCatalog:
    """Load a manifest catalog, or the old filename catalog when it is absent."""
    base = Path(root) if root is not None else Path(_persona_module.PERSONA_DIR)
    manifest = base / MANIFEST_NAME
    if manifest.is_symlink():
        raise PersonaCatalogError("persona manifest must not be a symlink")
    if not manifest.exists():
        descriptors = _legacy_descriptors(base)
        return PersonaCatalog("", descriptors, "legacy", base, MappingProxyType({}))
    default, descriptors = _parse_manifest(manifest)
    issues: dict[str, str] = {}
    for descriptor in descriptors:
        try:
            _load_bundle(base, descriptor)
        except PersonaCatalogError as exc:
            issues[descriptor.id] = str(exc)
    return PersonaCatalog(default, descriptors, "manifest", base, MappingProxyType(issues))


def _legacy_file(path: Path, identifier: str, component: str, root: Path) -> Path:
    try:
        mode = path.lstat().st_mode
    except OSError as exc:
        raise PersonaCatalogError(_safe_issue(identifier, component)) from exc
    if path.is_symlink() or not stat.S_ISREG(mode):
        raise PersonaCatalogError(_safe_issue(identifier, component))
    try:
        path.resolve().relative_to(root.resolve())
    except ValueError as exc:
        raise PersonaCatalogError(_safe_issue(identifier, component)) from exc
    return path


def _load_legacy_bundle(
    catalog: PersonaCatalog,
    descriptor: PersonaDescriptor,
    staging_path: Path | None = None,
    profiles_dir: Path | None = None,
    identity_override: Path | None = None,
) -> PersonaRuntime:
    candidates = (catalog.root / "identities" / descriptor.id, catalog.root / descriptor.id)
    candidate = identity_override or next(
        (path for path in candidates if path.exists() or path.is_symlink()), None
    )
    identity_path = (
        _legacy_file(
            candidate,
            descriptor.id,
            "identity.md",
            candidate.parent if identity_override else catalog.root,
        )
        if candidate
        else None
    )
    if identity_path is None:
        raise PersonaCatalogError(_safe_issue(descriptor.id, "identity.md"))
    behaviour_path = catalog.root / "behaviours" / "default.md"
    behaviour_path = _legacy_file(behaviour_path, descriptor.id, "default.md", catalog.root)
    from .progress import DEFAULT_LINES, load_staging_config

    staging_source = catalog.root / "behaviours" / "staging.yaml"
    if staging_path is not None:
        try:
            staging_source = _legacy_file(
                staging_path, descriptor.id, "staging.yaml", staging_path.parent
            )
        except PersonaCatalogError:
            staging_source = catalog.root / "behaviours" / "staging.yaml"
    if staging_source.exists() or staging_source.is_symlink():
        staging_source = _legacy_file(staging_source, descriptor.id, "staging.yaml", catalog.root)
        try:
            staging, _ = load_staging_config(staging_source)
        except (OSError, UnicodeError, StagingConfigError) as exc:
            raise PersonaCatalogError(_safe_issue(descriptor.id, "staging.yaml")) from exc
    else:
        staging = DEFAULT_LINES
    try:
        identity = identity_path.read_text(encoding="utf-8").strip()
        behaviour = behaviour_path.read_text(encoding="utf-8").strip()
    except (OSError, UnicodeError) as exc:
        raise PersonaCatalogError(_safe_issue(descriptor.id, "legacy component")) from exc
    if not identity or not behaviour:
        raise PersonaCatalogError(_safe_issue(descriptor.id, "legacy component"))
    bundle = PersonaBundle(
        descriptor.id,
        descriptor.label,
        identity,
        behaviour,
        staging,
        PersonaBundleSource(descriptor.id, identity_path, behaviour_path, staging_source),
    )
    return PersonaRuntime(bundle, _profile_staging(catalog.root, staging, profiles_dir))


def _profile_staging(
    root: Path, baseline: StagingLines, directory: Path | None = None
) -> Mapping[str, StagingLines]:
    directories = (
        [directory]
        if directory is not None
        else [
            root / "behaviours" / "staging",
            root / "behaviours" / "profiles" / "staging",
        ]
    )
    profiles: dict[str, StagingLines] = {}
    for directory in directories:
        try:
            entries = sorted(directory.iterdir())
        except OSError:
            continue
        for path in entries:
            if (
                path.name == "example.yaml"
                or not path.is_file()
                or path.suffix not in {".yaml", ".yml"}
            ):
                continue
            if path.stem in profiles:
                continue
            try:
                profiles[path.stem] = parse_profile_staging(
                    _load_yaml(path, "profile staging"), baseline
                )
            except (PersonaCatalogError, StagingConfigError, UnicodeError):
                log.warning("ignoring invalid profile staging %s (invalid content)", path)
                continue
    return MappingProxyType(profiles)


def prepare_runtime(
    catalog: PersonaCatalog,
    selection: str,
    staging_path: str | Path | None = None,
    profiles_dir: str | Path | None = None,
) -> PersonaRuntime:
    """Strictly prepare a selected runtime; this function never falls back."""
    descriptor = catalog.resolve(selection)
    if descriptor is None:
        raise PersonaCatalogError("unknown persona selection")
    if descriptor.id in catalog.issues:
        raise PersonaCatalogError(catalog.issues[descriptor.id])
    if catalog.mode == "legacy":
        return _load_legacy_bundle(
            catalog,
            descriptor,
            Path(staging_path) if staging_path else None,
            Path(profiles_dir) if profiles_dir else None,
        )
    bundle = _load_bundle(catalog.root, descriptor)
    return PersonaRuntime(bundle, _profile_staging(catalog.root, bundle.staging))


def load_example_bundle(root: str | Path | None = None) -> PersonaBundle:
    """Load the single complete tracked fallback bundle."""
    base = Path(root) if root is not None else Path(_persona_module.PERSONA_DIR)
    return _load_bundle(base, PersonaDescriptor(EXAMPLE_ID, "Kanade", ()))


def load_configured_runtime(
    catalog: PersonaCatalog | str | Path,
    selection: str | None,
    *,
    legacy_identity_path: str | Path | None = None,
    legacy_staging_path: str | Path | None = None,
    legacy_profiles_dir: str | Path | None = None,
) -> PersonaRuntime:
    """Load configured selection with whole-bundle default/example fallback."""
    root = Path(catalog) if isinstance(catalog, (str, Path)) else catalog.root
    try:
        loaded_catalog = load_catalog(catalog) if isinstance(catalog, (str, Path)) else catalog
    except PersonaCatalogError as exc:
        example = load_example_bundle(root)
        return PersonaRuntime(
            replace(example, fell_back=True, issue=str(exc)),
            _profile_staging(root, example.staging),
        )
    try:
        if selection:
            return prepare_runtime(
                loaded_catalog,
                selection,
                legacy_staging_path,
                legacy_profiles_dir,
            )
        raise PersonaCatalogError("no configured persona")
    except PersonaCatalogError as exc:
        if loaded_catalog.mode == "manifest":
            try:
                default = prepare_runtime(loaded_catalog, loaded_catalog.default_id)
                return PersonaRuntime(
                    replace(default.bundle, fell_back=True, issue=str(exc)), default.profile_staging
                )
            except PersonaCatalogError:
                pass
        if loaded_catalog.mode == "legacy" and legacy_identity_path:
            path = Path(legacy_identity_path)
            if selection == path.name and path.is_file() and not path.is_symlink():
                external = PersonaDescriptor(path.name, path.name, ())
                external_catalog = replace(loaded_catalog, descriptors=(external,))
                try:
                    # External identity is the legacy env compatibility exception;
                    # default/staging remain deployment-owned under the persona root.
                    runtime = _load_legacy_bundle(
                        external_catalog,
                        external,
                        Path(legacy_staging_path) if legacy_staging_path else None,
                        Path(legacy_profiles_dir) if legacy_profiles_dir else None,
                        path,
                    )
                except PersonaCatalogError:
                    pass
                else:
                    return runtime
        example = load_example_bundle(root)
        return PersonaRuntime(
            replace(example, fell_back=True, issue=str(exc)),
            _profile_staging(root, example.staging),
        )


__all__ = [
    "EXAMPLE_ID",
    "MANIFEST_NAME",
    "PERSONA_ROOT",
    "PersonaBundle",
    "PersonaBundleSource",
    "PersonaCatalog",
    "PersonaCatalogError",
    "PersonaDescriptor",
    "PersonaRuntime",
    "load_catalog",
    "load_configured_runtime",
    "load_example_bundle",
    "prepare_runtime",
]
