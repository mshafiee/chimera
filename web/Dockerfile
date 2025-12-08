# Multi-stage build for Chimera Web Dashboard
FROM node:18-alpine as builder

# Install build dependencies for native modules
RUN apk add --no-cache python3 make g++ libusb-dev linux-headers

# Set working directory
WORKDIR /app

# Copy package files
COPY package*.json ./

# Install dependencies (ignore scripts for optional native modules if they fail)
RUN npm install --legacy-peer-deps || (npm install --legacy-peer-deps --ignore-scripts && echo "Installed with --ignore-scripts due to native module issues")

# Copy source code
COPY . .

# Build the application
RUN npm run build

# Production stage
FROM nginx:alpine

# Copy built files from builder
COPY --from=builder /app/dist /usr/share/nginx/html

# Copy nginx configuration
COPY nginx.conf /etc/nginx/conf.d/default.conf

# Expose port
EXPOSE 80

# Health check
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
    CMD wget --quiet --tries=1 --spider http://localhost/ || exit 1

# Start nginx
CMD ["nginx", "-g", "daemon off;"]
