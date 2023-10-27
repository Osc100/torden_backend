### This is a web server to automate customer service.

It was for a hackathon with a really short dev time so the code is a bit messy, used to learn WebSockets in Rust. 

### To run the project.

First install Rust.
Then create a .env file with the following variables:
- OPENAI_API_KEY = A key to connect to OPENAI. 
- DATABASE_URL = a connection string to a PostgreSQL database.

Run the migrations:

```bash
cargo install --locked sqlx-cli
sqlx database create
sqlx migrate run
```

And then you can compile and run the project:

```bash
cargo run
```

If you have any questions feel free to ask or to open an issue!
