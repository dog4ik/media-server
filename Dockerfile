FROM rustlang/rust:nightly-slim
WORKDIR /app
ENV RESOURCES_PATH=/resources
ENV MOVIES_PATH=/videos/movies
ENV SHOWS_PATH=/videos/shows
ENV PORT=5000
COPY . .
RUN cargo build --release
RUN apt-get -y update
RUN apt-get -y upgrade
RUN apt-get install -y ffmpeg
CMD ["./target/release/media-server"]
EXPOSE 5000
