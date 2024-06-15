use inquire::{Text, Select, Confirm};
use std::fs::{create_dir_all, File};
use std::io::Write;
use std::path::Path;

fn main() {
    // Prompt for project details
    let project_name = Text::new("Project Name:")
        .with_default("my-app")
        .prompt()
        .unwrap();

    let file_formats = vec!["GraphQL", "JSON", "YML"];
    let file_format = Select::new("File Format:", file_formats)
        .prompt()
        .unwrap();

    // Determine file paths based on selected format
    let (config_directory, config_file_name) = match file_format {
        "GraphQL" => ("config", ".tailcallrc.graphql"),
        "JSON" => ("config", ".tailcallrc.schema.json"),
        "YML" => (".", ".graphqlrc.yml"),
        _ => unreachable!(),
    };

    // Summary and confirmation
    println!("Creating the following project structure:");
    println!("- {}/src/main.rs", project_name);
    println!("- {}/{}/{}", project_name, config_directory, config_file_name);

    let confirm = Confirm::new("Is this OK?")
        .with_default(true)
        .prompt()
        .unwrap();

    if confirm {
        // Create project directories and files
        let project_path = Path::new(&project_name);
        let src_path = project_path.join("src");
        let config_path = project_path.join(config_directory);

        create_dir_all(&src_path).unwrap();
        create_dir_all(&config_path).unwrap();

        // Create main.rs with Hello, World! GraphQL server
        let main_rs_path = src_path.join("main.rs");
        let mut main_rs_file = File::create(&main_rs_path).unwrap();
        writeln!(main_rs_file, r#"
use async_graphql::Context, Object, Schema, SimpleObject, EmptyMutation, EmptySubscription;

#[derive(SimpleObject)]
struct HelloWorld {{
    message: String,
}}

struct Query;

#[Object]
impl Query {{
    async fn hello(&self, _ctx: &Context<'_>) -> HelloWorld {{
        HelloWorld {{
            message: "Hello, World!".to_string(),
        }}
    }}
}}

#[tokio::main]
async fn main() {{
    let schema = Schema::build(Query, EmptyMutation, EmptySubscription).finish();
    println!("GraphQL Server running at http://localhost:8000");
}}
"#).unwrap();

        // Create configuration file with initial content
        let config_file_path = config_path.join(config_file_name);
        let mut config_file = File::create(&config_file_path).unwrap();
        
        match file_format{
            "GraphQL" => {
                let tailcallrc_graphql = include_str!("../../generated/.tailcallrc.graphql");
                writeln!(config_file, "{}", tailcallrc_graphql).unwrap();
            }
            "JSON" => {
                let tailcallrc_json = include_str!("../../generated/.tailcallrc.schema.json");
                writeln!(config_file, "{}", tailcallrc_json).unwrap();
            }
            "YML" => {
                let graphqlrc_yml = include_str!("../../.graphqlrc.yml");
                writeln!(config_file, "{}", graphqlrc_yml).unwrap();
            }
            _ => {}
        }

        println!("Project initialized successfully.");
    } else {
        println!("Initialization cancelled.");
    }
}
