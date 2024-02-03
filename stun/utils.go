package main

import (
	"fmt"
	"net/http"
	"regexp"
	"strings"
)

func routeParams(request *http.Request, pattern string) map[string]string {
	matches := regexp.MustCompile("{([^{}]+)}").FindAllStringSubmatch(pattern, -1)
	var keys []string
	for _, match := range matches {
		keys = append(keys, match[1])
	}
	newRegex := "^" + pattern + "$"
	for _, key := range keys {
		newRegex = strings.Replace(newRegex, "{"+key+"}", "(.*)", -1)
	}
	values := regexp.MustCompile(newRegex).FindStringSubmatch(request.URL.Path)[1:]

	if len(keys) != len(values) {
		panic("Could not parse route params. " + fmt.Sprint("Keys:", keys, "Values:", values))
	}
	params := make(map[string]string)
	for i, key := range keys {
		params[key] = values[i]
	}
	return params
}
