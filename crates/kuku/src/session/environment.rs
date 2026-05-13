use std::path::PathBuf;

use home::env::{self, Env};

use crate::error::{Error, Result};

pub fn kuku_home() -> Result<PathBuf> {
    kuku_home_with_env(&env::OS_ENV)
}

pub fn current_workspace() -> Result<PathBuf> {
    let cwd = std::env::current_dir()?;
    std::fs::canonicalize(&cwd).map_err(|_| Error::InvalidWorkspacePath(cwd.display().to_string()))
}

fn kuku_home_with_env(env: &dyn Env) -> Result<PathBuf> {
    if let Some(value) = env.var_os("KUKU_HOME") {
        if value.is_empty() {
            return Err(Error::InvalidKukuHome(String::new()));
        }
        return Ok(PathBuf::from(value));
    }

    env::home_dir_with_env(env)
        .map(|home| home.join(".kuku"))
        .ok_or(Error::MissingHomeDirectory)
}

#[cfg(test)]
mod tests {
    use super::kuku_home_with_env;
    use crate::error::Error;
    use home::env::Env;
    use std::collections::HashMap;
    use std::ffi::OsString;
    use std::io;
    use std::path::PathBuf;

    #[cfg(unix)]
    use std::os::unix::ffi::OsStringExt;

    #[derive(Default)]
    struct MockEnv {
        home_dir: Option<PathBuf>,
        vars: HashMap<&'static str, OsString>,
    }

    impl Env for MockEnv {
        fn home_dir(&self) -> Option<PathBuf> {
            self.home_dir.clone()
        }

        fn current_dir(&self) -> io::Result<PathBuf> {
            Ok(std::env::temp_dir())
        }

        fn var_os(&self, key: &str) -> Option<OsString> {
            self.vars.get(key).cloned()
        }
    }

    #[test]
    fn kuku_home_rejects_empty_kuku_home() {
        let mut env = MockEnv::default();
        env.vars.insert("KUKU_HOME", OsString::new());

        let error = kuku_home_with_env(&env).unwrap_err();

        assert!(matches!(error, Error::InvalidKukuHome(value) if value.is_empty()));
    }

    #[cfg(unix)]
    #[test]
    fn kuku_home_preserves_non_utf8_kuku_home() {
        let raw = OsString::from_vec(vec![b'/', b't', b'm', b'p', b'/', 0x80, b'h', b'o', b'm', b'e']);
        let mut env = MockEnv::default();
        env.vars.insert("KUKU_HOME", raw.clone());
        env.home_dir = Some(PathBuf::from("/should/not/use"));

        assert_eq!(kuku_home_with_env(&env).unwrap(), PathBuf::from(raw));
    }

    #[test]
    fn kuku_home_uses_platform_home_lookup_when_kuku_home_is_unset() {
        let env = MockEnv {
            home_dir: Some(PathBuf::from("/tmp/mock-home")),
            vars: HashMap::new(),
        };

        assert_eq!(
            kuku_home_with_env(&env).unwrap(),
            PathBuf::from("/tmp/mock-home").join(".kuku")
        );
    }

    #[cfg(unix)]
    #[test]
    fn kuku_home_uses_non_utf8_platform_home_losslessly() {
        let raw = OsString::from_vec(vec![b'/', b't', b'm', b'p', b'/', 0x80, b'u', b's', b'e', b'r']);
        let env = MockEnv {
            home_dir: Some(PathBuf::from(raw.clone())),
            vars: HashMap::new(),
        };

        assert_eq!(kuku_home_with_env(&env).unwrap(), PathBuf::from(raw).join(".kuku"));
    }

    #[test]
    fn kuku_home_returns_missing_home_directory_when_platform_lookup_fails() {
        let error = kuku_home_with_env(&MockEnv::default()).unwrap_err();

        assert!(matches!(error, Error::MissingHomeDirectory));
    }
}
