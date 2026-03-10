# syntax=docker/dockerfile:1
# Build
FROM node:23-alpine AS builder
WORKDIR /app

# corepack enables pnpm natively — no global npm install needed
RUN corepack enable && corepack prepare pnpm@latest --activate

COPY package.json pnpm-lock.yaml* ./
RUN --mount=type=cache,target=/root/.local/share/pnpm/store \
    pnpm install --frozen-lockfile || pnpm install

COPY . .
RUN pnpm build

# Serve
FROM nginx:alpine
COPY --from=builder /app/dist /usr/share/nginx/html
COPY nginx.conf /etc/nginx/conf.d/default.conf
EXPOSE 80
