FROM rustlang/rust:nightly-slim
WORKDIR /app
ENV RESOURCES_PATH=/resources
ENV LIBRARY_PATH=/videos
ENV PORT=5000
COPY . .
RUN cargo build --release
RUN apt-get -y update
RUN apt-get -y upgrade
RUN apt-get install -y ffmpeg
CMD ["./target/release/media-server"]
EXPOSE 5000
