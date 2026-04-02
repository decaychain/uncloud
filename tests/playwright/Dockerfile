FROM mcr.microsoft.com/playwright:v1.58.2-noble

WORKDIR /app

COPY package.json package-lock.json* ./
RUN npm install

COPY playwright.config.ts global-setup.ts ./
COPY tests/ tests/

CMD ["npx", "playwright", "test"]
