build:
	GOARCH=arm64 GOOS=linux go build -tags lambda.norpc -o ./bin/bootstrap
	cd bin && zip -FS bootstrap.zip bootstrap