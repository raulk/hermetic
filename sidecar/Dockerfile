FROM node:22-bookworm-slim

WORKDIR /app

COPY package.json package-lock.json ./
RUN npm install --omit=dev --ignore-scripts

COPY main.mjs ./

USER node
ENTRYPOINT ["node", "/app/main.mjs"]
