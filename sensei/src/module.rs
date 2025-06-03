pub trait Run<ServiceConfig> {
    fn new(config: ServiceConfig) -> Self;
    async fn run(&self, config: ServiceConfig) -> Result<(), Box<dyn std::error::Error>>;
}
