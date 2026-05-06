//! Deno worker plumbing required by `deno_runtime` even when no npm
//! packages are present: a `NodeResolver` instance must be in the
//! `OpState`, and the worker pulls a require loader plus a CJS-detection
//! hook.

use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use anyhow::Result;
use deno_error::JsErrorBox;
use deno_runtime::deno_core::{extension, FastString};
use deno_runtime::deno_node::{NodeRequireLoader, NodeRequireLoaderRc};
use deno_runtime::deno_permissions::{OpenAccessKind, PermissionsContainer};

#[derive(Debug)]
pub struct NoNpm;

impl node_resolver::InNpmPackageChecker for NoNpm {
    fn in_npm_package(&self, _specifier: &deno_runtime::deno_core::url::Url) -> bool {
        false
    }
}

impl node_resolver::NpmPackageFolderResolver for NoNpm {
    fn resolve_package_folder_from_package(
        &self,
        specifier: &str,
        referrer: &node_resolver::UrlOrPathRef,
    ) -> Result<PathBuf, node_resolver::errors::PackageFolderResolveError> {
        Err(node_resolver::errors::PackageFolderResolveError(Box::new(
            node_resolver::errors::PackageFolderResolveErrorKind::PackageNotFound(
                node_resolver::errors::PackageNotFoundError {
                    package_name: specifier.to_string(),
                    referrer: referrer.display(),
                    referrer_extra: None,
                },
            ),
        )))
    }

    fn resolve_types_package_folder(
        &self,
        _types_package_name: &str,
        _maybe_package_version: Option<&deno_semver::Version>,
        _maybe_referrer: Option<&node_resolver::UrlOrPathRef>,
    ) -> Option<PathBuf> {
        None
    }
}

#[derive(Debug)]
struct LocalNodeRequireLoader;

impl NodeRequireLoader for LocalNodeRequireLoader {
    fn ensure_read_permission<'a>(
        &self,
        permissions: &mut PermissionsContainer,
        path: Cow<'a, Path>,
    ) -> Result<Cow<'a, Path>, JsErrorBox> {
        permissions
            .check_open(path, OpenAccessKind::Read, Some("node:require"))
            .map(deno_runtime::deno_permissions::CheckedPath::into_path)
            .map_err(JsErrorBox::from_err)
    }

    fn load_text_file_lossy(&self, path: &Path) -> Result<FastString, JsErrorBox> {
        std::fs::read_to_string(path)
            .map(Into::into)
            .map_err(|err| JsErrorBox::generic(err.to_string()))
    }

    fn is_maybe_cjs(
        &self,
        _specifier: &deno_runtime::deno_core::url::Url,
    ) -> Result<bool, node_resolver::errors::PackageJsonLoadError> {
        Ok(true)
    }
}

extension!(
    hermetic_node_state,
    state = |state| {
        let sys = sys_traits::impls::RealSys;
        let pkg_json_resolver =
            Arc::new(node_resolver::PackageJsonResolver::new(sys.clone(), None));
        let node_resolver = Arc::new(node_resolver::NodeResolver::new(
            NoNpm,
            node_resolver::DenoIsBuiltInNodeModuleChecker,
            NoNpm,
            pkg_json_resolver.clone(),
            node_resolver::cache::NodeResolutionSys::new(sys.clone(), None),
            node_resolver::NodeResolverOptions {
                conditions: node_resolver::NodeConditionOptions::default(),
                is_browser_platform: false,
                bundle_mode: false,
                typescript_version: None,
            },
        ));
        state.put(sys);
        state.put::<NodeRequireLoaderRc>(Rc::new(LocalNodeRequireLoader));
        state.put(pkg_json_resolver);
        state.put(node_resolver);
    }
);
