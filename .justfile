SERVER_HOST := "ubuntu@api.torden.services"
KEY := "~/.ssh/torden.pem"
RELEASE_FILES := "/home/osc/Projects/torden_backend/target/release/*"
# Define the Rust project's root directory.
# Update this with your project's actual path.

# Build the Rust project using `cargo build --release`.

# Define a command to copy the files to the server using `scp`.
# This command will use the `build` target to ensure the project is built first.
# 

push: 
    git push

pull: push
    ssh -i {{KEY}} {{SERVER_HOST}} cd /home/ubuntu/repos/torden_backend && git pull

build: pull
    ssh -i {{KEY}} {{SERVER_HOST}} cd /home/ubuntu/repos/torden_backend && cargo build --release
    
deploy: build
    ssh -i {{KEY}} {{SERVER_HOST}} "sudo systemctl restart axum"


