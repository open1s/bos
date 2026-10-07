use super::*;

impl<A: ReActApp> ReActEngine<A> {
    /// Whether an LLM error is worth retrying.
    ///
    /// Delegates to [`LlmError::is_retryable`], which classifies by variant
    /// and HTTP status rather than by matching on the debug text.
    fn is_transient_error(err: &LlmError) -> bool {
        err.is_retryable()
    }

    /// Call LLM with optional resilience wrapper and retry on transient errors.
    pub async fn call_llm(
        &mut self,
        persona: Option<String>,
        request: LlmRequest,
        session: &mut A::Session,
        context: &mut A::Context,
    ) -> Result<LlmResponse, ReactError>
    where
        A::Session: ReactSession,
    {
        let max_retries = self
            .resilience
            .as_ref()
            .map(|r| r.rate_limit_config().max_retries)
            .unwrap_or(3);

        let mut attempt = 0;
        let t0 = std::time::Instant::now();

        loop {
            let t_iter = std::time::Instant::now();
            let result = if let Some(resilience) = &self.resilience {
                resilience.acquire().await.map_err(ReactError::from)?;
                resilience.check_circuit().map_err(ReactError::from)?;
                self.llm
                    .complete(persona.clone(), request.clone(), session, context)
                    .await
            } else {
                self.llm
                    .complete(persona.clone(), request.clone(), session, context)
                    .await
            };
            info!(
                "[TIMING] call_llm attempt {}: {:?}",
                attempt,
                t_iter.elapsed()
            );

            // Record outcome in circuit breaker so it learns from actual LLM results
            if let Some(ref resilience) = self.resilience {
                match &result {
                    Ok(_) => resilience.record_success(),
                    Err(_) => resilience.record_failure(),
                }
            }

            if let Some(usage) = result.as_ref().ok().and_then(|r| r.usage()) {
                self.token_counter.update_from_response(usage);
            }

            // If successful, return
            if result.is_ok() {
                info!(
                    "[TIMING] call_llm total (attempt {}): {:?}",
                    attempt,
                    t0.elapsed()
                );
                return result.map_err(ReactError::from);
            }

            // Check if error is transient and we should retry
            let should_retry = if let Err(ref err) = result {
                Self::is_transient_error(err)
            } else {
                false
            };

            if !should_retry {
                info!(
                    "[TIMING] call_llm total (non-retry error): {:?}",
                    t0.elapsed()
                );
                return result.map_err(ReactError::from);
            }

            // Check if we should retry
            attempt += 1;
            if attempt >= max_retries {
                info!("[TIMING] call_llm total (max retries): {:?}", t0.elapsed());
                return result.map_err(ReactError::from);
            }

            // Exponential backoff from the configured base, capped at 64x.
            let backoff = self
                .resilience
                .as_ref()
                .map(|r| r.rate_limit_config().retry_backoff)
                .unwrap_or(std::time::Duration::from_millis(500));
            let delay = backoff * (1u32 << (attempt - 1)).min(6);
            info!("[TIMING] call_llm retrying after {:?} delay", delay);
            match &self.resilience {
                Some(resilience) => resilience.backoff(delay).await,
                None => tokio::time::sleep(delay).await,
            }
        }
    }

    /// Call LLM for streaming with optional resilience wrapper.
    /// Returns an owned stream that doesn't borrow from self, allowing tool calls
    /// to be executed immediately within the stream loop.
    pub async fn call_llm_stream(
        &self,
        persona: Option<String>,
        request: LlmRequest,
        session: &mut A::Session,
        context: &mut A::Context,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamToken, LlmError>> + Send>>, ReactError>
    where
        A::Session: ReactSession,
    {
        let result = if let Some(resilience) = &self.resilience {
            resilience.acquire().await.map_err(ReactError::from)?;
            resilience.check_circuit().map_err(ReactError::from)?;

            self.llm
                .stream_complete(persona.clone(), request, session, context)
                .await
        } else {
            self.llm
                .stream_complete(persona.clone(), request, session, context)
                .await
        };

        // Record outcome in circuit breaker so it learns from actual LLM results
        if let Some(ref resilience) = self.resilience {
            match &result {
                Ok(_) => resilience.record_success(),
                Err(_) => resilience.record_failure(),
            }
        }

        result.map_err(ReactError::from)
    }
}
