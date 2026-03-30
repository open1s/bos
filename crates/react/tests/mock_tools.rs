#[allow(dead_code)]
pub struct WeatherAPIMock;
impl WeatherAPIMock {
    pub fn new() -> Self {
        WeatherAPIMock
    }
    pub fn call(&self) -> i32 {
        42
    }
}

impl Default for WeatherAPIMock {
    fn default() -> Self {
        Self::new()
    }
}
