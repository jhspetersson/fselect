FROM rust:latest

WORKDIR /usr/src/fselect
COPY . .

RUN cargo install --locked --path .

CMD ["cargo", "test", "--locked" , "--verbose", "--all"]
