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

            // `max_retries` counts retries after the first attempt, matching
            // `RateLimiterConfig` and `ReActResilience::execute_with`. Compare
            // before incrementing so a limit of 3 retries makes four calls.
            if attempt >= max_retries {
                info!("[TIMING] call_llm total (max retries): {:?}", t0.elapsed());
                return result.map_err(ReactError::from);
            }
            attempt += 1;

            // Exponential backoff from the configured base, capped at 6x so
            // a large retry budget cannot overflow the shift or the delay.
            let backoff = self
                .resilience
                .as_ref()
                .map(|r| r.rate_limit_config().retry_backoff)
                .unwrap_or(std::time::Duration::from_millis(500));
            let factor = 1u32.checked_shl(attempt - 1).unwrap_or(u32::MAX).min(6);
            let delay = backoff * factor;
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
        // The same retry contract as `call_llm`: a provider that answers 429 or
        // 503 before the stream opens is transient, and the streaming path is
        // the one the interactive turn uses. Measured: a quota 429 failed the
        // turn in 1.5 s, six times in a row, while the retry loop sat unused on
        // the non-streaming path.
        let max_retries = self
            .resilience
            .as_ref()
            .map(|r| r.rate_limit_config().max_retries)
            .unwrap_or(3);
        let backoff = self
            .resilience
            .as_ref()
            .map(|r| r.rate_limit_config().retry_backoff)
            .unwrap_or(std::time::Duration::from_millis(500));

        let mut attempt = 0u32;
        loop {
            let result = if let Some(resilience) = &self.resilience {
                resilience.acquire().await.map_err(ReactError::from)?;
                resilience.check_circuit().map_err(ReactError::from)?;

                self.llm
                    .stream_complete(persona.clone(), request.clone(), session, context)
                    .await
            } else {
                self.llm
                    .stream_complete(persona.clone(), request.clone(), session, context)
                    .await
            };

            // Record outcome in circuit breaker so it learns from actual LLM results
            if let Some(ref resilience) = self.resilience {
                match &result {
                    Ok(_) => resilience.record_success(),
                    Err(_) => resilience.record_failure(),
                }
            }

            match result {
                // A stream that opened is returned as-is: a failure AFTER it
                // opens is reported through the stream, where a retry would
                // duplicate the text already shown.
                Ok(stream) => return Ok(stream),
                Err(error) => {
                    if !Self::is_transient_error(&error) || attempt >= max_retries {
                        return Err(ReactError::from(error));
                    }
                    attempt += 1;
                    // The same exponential backoff `call_llm` uses, so both
                    // paths wait the same way on the same provider.
                    let factor = 1u32.checked_shl(attempt - 1).unwrap_or(u32::MAX).min(6);
                    let delay = backoff * factor;
                    info!("[TIMING] call_llm_stream retrying after {:?}", delay);
                    match &self.resilience {
                        Some(resilience) => resilience.backoff(delay).await,
                        None => tokio::time::sleep(delay).await,
                    }
                }
            }
        }
    }
}
