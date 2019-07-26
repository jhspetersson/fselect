FROM rust:latest

WORKDIR /usr/src/fselect
COPY . .

RUN cargo install --path .

CMD ["cargo", "test", "--verbose", "--all"]
