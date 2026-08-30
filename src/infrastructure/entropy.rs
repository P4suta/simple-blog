use rand::{TryRng as _, rngs::SysRng};

use crate::application::ports::{EntropyError, EntropySource};

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemEntropy;

impl EntropySource for SystemEntropy {
    fn fill(&self, destination: &mut [u8]) -> Result<(), EntropyError> {
        SysRng.try_fill_bytes(destination).map_err(|error| {
            tracing::error!(event = "auth.entropy_unavailable", %error);
            EntropyError
        })
    }
}
