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
