# Code Review Checklist

Use this checklist during code reviews to ensure comprehensive coverage.

## Security Checklist

### Input Validation
- [ ] All user inputs are validated against expected schemas
- [ ] Database queries use parameterized statements (no concatenation)
- [ ] File paths are validated and sanitized
- [ ] API inputs are validated before processing

### Authentication & Authorization
- [ ] Authentication is enforced on sensitive endpoints
- [ ] Authorization checks exist for resource access
- [ ] Role-based access control is properly implemented
- [ ] Session tokens are validated

### Data Protection
- [ ] Secrets are never logged or exposed
- [ ] Sensitive data is encrypted at rest
- [ ] HTTPS is used for data in transit
- [ ] PII is handled according to privacy requirements

## Performance Checklist

### Database
- [ ] Queries use appropriate indexes
- [ ] No N+1 query patterns detected
- [ ] Large datasets use pagination
- [ ] Expensive queries are cached

### Memory & Resources
- [ ] No obvious memory leaks
- [ ] Resources are properly cleaned up
- [ ] Connection pools are configured appropriately
- [ ] Large objects are handled efficiently

### Algorithms
- [ ] Algorithmic complexity is acceptable
- [ ] No unnecessary iterations
- [ ] Caching is used where appropriate
- [ ] Async/await is used for I/O operations

## Code Quality Checklist

### Naming & Structure
- [ ] Function and variable names are descriptive
- [ ] Functions are focused and single-purpose
- [ ] Code is not deeply nested
- [ ] No duplicated code blocks

### Error Handling
- [ ] All error paths are handled
- [ ] Error messages are helpful
- [ ] Errors are logged appropriately
- [ ] Graceful degradation where possible

### Testing
- [ ] Unit tests cover critical paths
- [ ] Edge cases are tested
- [ ] Tests are readable and maintainable
- [ ] Test names describe what is being tested

### Documentation
- [ ] Complex logic has explanatory comments
- [ ] Public APIs are documented
- [ ] README is up to date
- [ ] Architecture decisions are documented
